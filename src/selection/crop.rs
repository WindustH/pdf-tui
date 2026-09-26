//! Rendering a selection on its own: a crop of the page at the size the
//! selection view or the clipboard needs, cached under `selection/`.

use std::path::{Path, PathBuf};

use image::{GenericImageView, ImageFormat};
use sha2::{Digest, Sha256};

use crate::{
  cache,
  pdf::{PageImage, PdfDocument},
};

use super::{PdfRect, PdfSelection, hash_pdf_rect, page_image_from_path};

pub async fn render_selection_preview_image(
  document: PdfDocument,
  selection: PdfSelection,
  cache_dir: PathBuf,
  target_width: u32,
  target_height: u32,
  cache_max_bytes: u64,
) -> Result<PageImage, String> {
  render_selection_image(SelectionImageRenderRequest {
    document,
    selection,
    cache_dir,
    target_width,
    target_height,
    max_pixels: None,
    label: "preview",
    cache_max_bytes,
  })
  .await
}

pub async fn render_selection_copy_image(
  document: PdfDocument,
  selection: PdfSelection,
  cache_dir: PathBuf,
  max_pixels: u64,
  cache_max_bytes: u64,
) -> Result<PathBuf, String> {
  let (target_width, target_height) = selection_copy_page_target(selection, max_pixels);
  render_selection_image(SelectionImageRenderRequest {
    document,
    selection,
    cache_dir,
    target_width,
    target_height,
    max_pixels: Some(max_pixels),
    label: "copy",
    cache_max_bytes,
  })
  .await
  .map(|image| image.path)
}

struct SelectionImageRenderRequest {
  document: PdfDocument,
  selection: PdfSelection,
  cache_dir: PathBuf,
  target_width: u32,
  target_height: u32,
  max_pixels: Option<u64>,
  label: &'static str,
  cache_max_bytes: u64,
}

async fn render_selection_image(request: SelectionImageRenderRequest) -> Result<PageImage, String> {
  let SelectionImageRenderRequest {
    document,
    selection,
    cache_dir,
    target_width,
    target_height,
    max_pixels,
    label,
    cache_max_bytes,
  } = request;
  let target_width = target_width.max(1);
  let target_height = target_height.max(1);
  let dir = cache_dir.join("selection");
  tokio::fs::create_dir_all(&dir)
    .await
    .map_err(|error| error.to_string())?;
  let path = dir.join(format!(
    "{}.png",
    document_selection_cache_key(
      &document,
      selection,
      target_width,
      target_height,
      max_pixels,
      label,
    )
  ));
  if path.exists() {
    cache::touch_cache_entry(&path).await;
    return page_image_from_path(selection.page_index, path, None);
  }
  let _lock = cache::acquire_cache_file_lock(&path)
    .await
    .map_err(|error| format!("failed to lock selection cache {}: {error}", path.display()))?;
  if path.exists() {
    cache::touch_cache_entry(&path).await;
    return page_image_from_path(selection.page_index, path, None);
  }
  if let Ok(crop) = crate::pdf::render_uncached_selection_image_at(
    &document,
    selection,
    target_width,
    target_height,
  )
  .await
  {
    cache::copy_file_atomic(&crop.image().path, &path)
      .await
      .map_err(|error| {
        format!(
          "failed to copy selection crop {} to {}: {error}",
          crop.image().path.display(),
          path.display()
        )
      })?;
    cache::touch_cache_entry(&path).await;
    let _ = cache::enforce_cache_target_limit_sync(&cache_dir, &dir, cache_max_bytes);
    return page_image_from_path(selection.page_index, path, None);
  }
  let (fallback_page_width, fallback_page_height) =
    selection_fallback_page_target(selection, target_width, target_height);
  let page = crate::pdf::render_uncached_page_image_at(
    &document,
    selection.page_index,
    fallback_page_width,
    fallback_page_height,
  )
  .await
  .map_err(|error| error.to_string())?;
  let path_for_worker = path.clone();
  tokio::task::spawn_blocking(move || {
    write_cropped_page(&path_for_worker, page.image(), selection, max_pixels)
  })
  .await
  .map_err(|error| format!("selection image worker failed: {error}"))??;
  cache::touch_cache_entry(&path).await;
  let _ = cache::enforce_cache_target_limit_sync(&cache_dir, &dir, cache_max_bytes);
  page_image_from_path(selection.page_index, path, None)
}

pub fn selection_image_cache_key(
  document: &PdfDocument,
  selection: PdfSelection,
  target_width: u32,
  target_height: u32,
  label: &str,
) -> String {
  document_selection_cache_key(
    document,
    selection,
    target_width.max(1),
    target_height.max(1),
    None,
    label,
  )
}

pub fn selection_preview_page_target(
  selection: PdfSelection,
  area_width: u16,
  area_height: u16,
  cell_pixels: Option<(u16, u16)>,
) -> (u32, u32) {
  let (cell_width, cell_height) = cell_pixels.unwrap_or((8, 16));
  let max_crop_width = u32::from(area_width.max(1)).saturating_mul(u32::from(cell_width.max(1)));
  let max_crop_height = u32::from(area_height.max(1)).saturating_mul(u32::from(cell_height.max(1)));
  let rect_width = selection.rect.width().max(1.0);
  let rect_height = selection.rect.height().max(1.0);
  let scale = (f64::from(max_crop_width.max(1)) / rect_width)
    .min(f64::from(max_crop_height.max(1)) / rect_height);
  crop_target_from_scale(selection, scale)
}

pub fn selection_copy_page_target(selection: PdfSelection, max_pixels: u64) -> (u32, u32) {
  let scale = selection_scale_for_crop_pixels(selection, max_pixels);
  crop_target_from_scale(selection, scale)
}

fn write_cropped_page(
  path: &Path,
  page: &PageImage,
  selection: PdfSelection,
  max_pixels: Option<u64>,
) -> Result<(), String> {
  let image = image::open(&page.path)
    .map_err(|error| format!("failed to open {}: {error}", page.path.display()))?;
  let (width, height) = image.dimensions();
  let rect = pdf_rect_to_full_page_pixels(selection.page_size(), selection.rect, width, height);
  let Some(rect) = rect else {
    return Err("selection is outside rendered page".to_string());
  };
  let mut cropped = image.crop_imm(rect.x, rect.y, rect.width, rect.height);
  if let Some(max_pixels) = max_pixels {
    cropped = downscale_to_pixel_limit(cropped, max_pixels);
  }
  cache::write_file_atomic_sync(path, |temp| {
    cropped.save_with_format(temp, ImageFormat::Png)
  })
}

fn downscale_to_pixel_limit(image: image::DynamicImage, max_pixels: u64) -> image::DynamicImage {
  let (width, height) = image.dimensions();
  let pixels = u64::from(width.max(1)).saturating_mul(u64::from(height.max(1)));
  if pixels <= max_pixels.max(1) {
    return image;
  }
  let scale = (max_pixels.max(1) as f64 / pixels as f64).sqrt();
  let target_width = (f64::from(width) * scale).round().max(1.0) as u32;
  let target_height = (f64::from(height) * scale).round().max(1.0) as u32;
  image.resize(
    target_width,
    target_height,
    image::imageops::FilterType::Lanczos3,
  )
}

#[derive(Debug, Clone, Copy)]
struct PixelRect {
  x: u32,
  y: u32,
  width: u32,
  height: u32,
}

fn pdf_rect_to_full_page_pixels(
  page_size: (u32, u32),
  rect: PdfRect,
  pixel_width: u32,
  pixel_height: u32,
) -> Option<PixelRect> {
  let page_width = f64::from(page_size.0.max(1));
  let page_height = f64::from(page_size.1.max(1));
  let rect = rect.clamp_to_page(page_width, page_height);
  if rect.is_empty() {
    return None;
  }
  let x_scale = f64::from(pixel_width.max(1)) / page_width;
  let y_scale = f64::from(pixel_height.max(1)) / page_height;
  let x0 = (rect.x_min * x_scale).floor() as i64;
  let y0 = (rect.y_min * y_scale).floor() as i64;
  let x1 = (rect.x_max * x_scale).ceil() as i64;
  let y1 = (rect.y_max * y_scale).ceil() as i64;
  let x0 = x0.clamp(0, i64::from(pixel_width.saturating_sub(1))) as u32;
  let y0 = y0.clamp(0, i64::from(pixel_height.saturating_sub(1))) as u32;
  let x1 = x1.clamp(i64::from(x0.saturating_add(1)), i64::from(pixel_width)) as u32;
  let y1 = y1.clamp(i64::from(y0.saturating_add(1)), i64::from(pixel_height)) as u32;
  Some(PixelRect {
    x: x0,
    y: y0,
    width: x1.saturating_sub(x0).max(1),
    height: y1.saturating_sub(y0).max(1),
  })
}

fn selection_scale_for_crop_pixels(selection: PdfSelection, max_pixels: u64) -> f64 {
  let rect_area = selection.rect.width().max(1.0) * selection.rect.height().max(1.0);
  (max_pixels.max(1) as f64 / rect_area).sqrt().max(0.01)
}

fn crop_target_from_scale(selection: PdfSelection, scale: f64) -> (u32, u32) {
  let rect_width = selection.rect.width().max(1.0);
  let rect_height = selection.rect.height().max(1.0);
  let scale = scale.max(0.01);
  let width = (rect_width * scale).round().clamp(1.0, f64::from(u32::MAX)) as u32;
  let height = (rect_height * scale)
    .round()
    .clamp(1.0, f64::from(u32::MAX)) as u32;
  (width.max(1), height.max(1))
}

fn selection_fallback_page_target(
  selection: PdfSelection,
  crop_width: u32,
  crop_height: u32,
) -> (u32, u32) {
  let x_scale = f64::from(crop_width.max(1)) / selection.rect.width().max(1.0);
  let y_scale = f64::from(crop_height.max(1)) / selection.rect.height().max(1.0);
  let width = (selection.page_width.max(1.0) * x_scale.max(0.01))
    .ceil()
    .clamp(1.0, f64::from(u32::MAX)) as u32;
  let height = (selection.page_height.max(1.0) * y_scale.max(0.01))
    .ceil()
    .clamp(1.0, f64::from(u32::MAX)) as u32;
  (width.max(1), height.max(1))
}

fn document_selection_cache_key(
  document: &PdfDocument,
  selection: PdfSelection,
  target_width: u32,
  target_height: u32,
  max_pixels: Option<u64>,
  label: &str,
) -> String {
  let mut hasher = Sha256::new();
  hasher.update(b"pdf-tui-selection-render-v4");
  hasher.update(label.as_bytes());
  hasher.update([0]);
  hasher.update(document.raster_backend.label().as_bytes());
  hasher.update([0]);
  hasher.update(document.path.to_string_lossy().as_bytes());
  hasher.update(document.size_bytes.to_le_bytes());
  hasher.update(document.modified_nanos.to_le_bytes());
  hasher.update(document.dpi.to_le_bytes());
  hasher.update(selection.page_index.to_le_bytes());
  hasher.update(selection.page_width.to_le_bytes());
  hasher.update(selection.page_height.to_le_bytes());
  hash_pdf_rect(&mut hasher, selection.rect);
  hasher.update(target_width.to_le_bytes());
  hasher.update(target_height.to_le_bytes());
  hasher.update(max_pixels.unwrap_or_default().to_le_bytes());
  hex::encode(hasher.finalize())
}
