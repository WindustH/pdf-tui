use std::{
  fs,
  path::{Path, PathBuf},
  time::UNIX_EPOCH,
};

use image::{GenericImageView, ImageFormat, RgbaImage};
use sha2::{Digest, Sha256};

use crate::{
  cache,
  pdf::{PageImage, PdfDocument},
};

const DEFAULT_TERMINAL_CELL_ASPECT: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PdfPoint {
  pub x: f64,
  pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PdfRect {
  pub x_min: f64,
  pub y_min: f64,
  pub x_max: f64,
  pub y_max: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PdfSelection {
  pub page_index: usize,
  pub page_width: f64,
  pub page_height: f64,
  pub rect: PdfRect,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelectionAnchor {
  pub page_index: usize,
  pub page_width: f64,
  pub page_height: f64,
  pub point: PdfPoint,
  pub marker: PdfRect,
}

impl PdfRect {
  pub fn intersection(self, other: Self) -> Option<Self> {
    let rect = Self {
      x_min: self.x_min.max(other.x_min),
      y_min: self.y_min.max(other.y_min),
      x_max: self.x_max.min(other.x_max),
      y_max: self.y_max.min(other.y_max),
    }
    .normalized();
    (!rect.is_empty()).then_some(rect)
  }

  pub fn clamp_to_page(self, page_width: f64, page_height: f64) -> Self {
    let page_width = page_width.max(1.0);
    let page_height = page_height.max(1.0);
    Self {
      x_min: self.x_min.clamp(0.0, page_width),
      y_min: self.y_min.clamp(0.0, page_height),
      x_max: self.x_max.clamp(0.0, page_width),
      y_max: self.y_max.clamp(0.0, page_height),
    }
    .normalized()
  }

  pub fn normalized(self) -> Self {
    Self {
      x_min: self.x_min.min(self.x_max),
      y_min: self.y_min.min(self.y_max),
      x_max: self.x_min.max(self.x_max),
      y_max: self.y_min.max(self.y_max),
    }
  }

  pub fn width(self) -> f64 {
    (self.x_max - self.x_min).max(0.0)
  }

  pub fn height(self) -> f64 {
    (self.y_max - self.y_min).max(0.0)
  }

  pub fn is_empty(self) -> bool {
    self.width() <= f64::EPSILON || self.height() <= f64::EPSILON
  }
}

impl PdfSelection {
  pub fn page_size(self) -> (u32, u32) {
    (
      self.page_width.round().clamp(1.0, f64::from(u32::MAX)) as u32,
      self.page_height.round().clamp(1.0, f64::from(u32::MAX)) as u32,
    )
  }
}

pub fn marker_page_image(
  cache_dir: &Path,
  page: &PageImage,
  page_size: (u32, u32),
  marker: PdfRect,
  max_bytes: u64,
) -> Result<PageImage, String> {
  transformed_page_image(
    cache_dir,
    page,
    page_size,
    marker,
    "marker-crosshair-v2",
    max_bytes,
    |image, rect| {
      invert_crosshair(image, rect);
    },
  )
}

pub fn outline_page_image(
  cache_dir: &Path,
  page: &PageImage,
  page_size: (u32, u32),
  outline: PdfRect,
  max_bytes: u64,
) -> Result<PageImage, String> {
  transformed_page_image(
    cache_dir,
    page,
    page_size,
    outline,
    "selection-outline-v1",
    max_bytes,
    |image, rect| {
      invert_outline(image, rect);
    },
  )
}

pub fn marker_selection_crop_image(
  cache_dir: &Path,
  crop: &PageImage,
  selection: PdfSelection,
  marker: PdfRect,
  max_bytes: u64,
) -> Result<PageImage, String> {
  if marker.intersection(selection.rect).is_none() {
    return Ok(crop.clone());
  }
  transformed_crop_image(
    cache_dir,
    crop,
    selection,
    marker,
    "selection-marker-crosshair-v2",
    max_bytes,
    |image, rect| {
      invert_crosshair(image, rect);
    },
  )
}

pub fn outline_selection_crop_image(
  cache_dir: &Path,
  crop: &PageImage,
  selection: PdfSelection,
  outline: PdfRect,
  max_bytes: u64,
) -> Result<PageImage, String> {
  if outline.intersection(selection.rect).is_none() {
    return Ok(crop.clone());
  }
  transformed_crop_image(
    cache_dir,
    crop,
    selection,
    outline,
    "selection-crop-outline-v1",
    max_bytes,
    |image, rect| {
      invert_outline(image, rect);
    },
  )
}

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
  if let Ok(crop) = crate::pdf::render_uncached_selection_image_at(
    &document,
    selection,
    target_width,
    target_height,
  )
  .await
  {
    tokio::fs::copy(&crop.image().path, &path)
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

fn transformed_page_image(
  cache_dir: &Path,
  page: &PageImage,
  page_size: (u32, u32),
  rect: PdfRect,
  label: &str,
  max_bytes: u64,
  transform: impl FnOnce(&mut RgbaImage, MarkerPixelRect),
) -> Result<PageImage, String> {
  let dir = cache_dir.join("selection");
  fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
  let path = dir.join(format!(
    "{}.png",
    transformed_cache_key(page, page_size, rect, label)
  ));
  if !path.exists() {
    write_transformed_page(&path, page, page_size, rect, transform)?;
    let _ = cache::enforce_cache_target_limit_sync(cache_dir, &dir, max_bytes);
  }
  cache::touch_cache_entry_sync(&path);
  page_image_from_path(page.page_index, path, page.slice.clone())
}

fn transformed_crop_image(
  cache_dir: &Path,
  crop: &PageImage,
  selection: PdfSelection,
  marker: PdfRect,
  label: &str,
  max_bytes: u64,
  transform: impl FnOnce(&mut RgbaImage, MarkerPixelRect),
) -> Result<PageImage, String> {
  let dir = cache_dir.join("selection");
  fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
  let path = dir.join(format!(
    "{}.png",
    transformed_crop_cache_key(crop, selection, marker, label)
  ));
  if !path.exists() {
    write_transformed_crop(&path, crop, selection, marker, transform)?;
    let _ = cache::enforce_cache_target_limit_sync(cache_dir, &dir, max_bytes);
  }
  cache::touch_cache_entry_sync(&path);
  page_image_from_path(crop.page_index, path, None)
}

fn write_transformed_page(
  path: &Path,
  page: &PageImage,
  page_size: (u32, u32),
  rect: PdfRect,
  transform: impl FnOnce(&mut RgbaImage, MarkerPixelRect),
) -> Result<(), String> {
  let image = image::open(&page.path)
    .map_err(|error| format!("failed to open {}: {error}", page.path.display()))?;
  let (width, height) = image.dimensions();
  let mut image = image.to_rgba8();
  if let Some(pixel_rect) = pdf_rect_to_source_marker_pixels(page, page_size, rect, width, height) {
    transform(&mut image, pixel_rect);
  }
  image
    .save_with_format(path, ImageFormat::Png)
    .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn write_transformed_crop(
  path: &Path,
  crop: &PageImage,
  selection: PdfSelection,
  marker: PdfRect,
  transform: impl FnOnce(&mut RgbaImage, MarkerPixelRect),
) -> Result<(), String> {
  let image = image::open(&crop.path)
    .map_err(|error| format!("failed to open {}: {error}", crop.path.display()))?;
  let (width, height) = image.dimensions();
  let mut image = image.to_rgba8();
  if let Some(pixel_rect) =
    pdf_rect_to_selection_crop_marker_pixels(selection, marker, width, height)
  {
    transform(&mut image, pixel_rect);
  }
  image
    .save_with_format(path, ImageFormat::Png)
    .map_err(|error| format!("failed to write {}: {error}", path.display()))
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
  cropped
    .save_with_format(path, ImageFormat::Png)
    .map_err(|error| format!("failed to write {}: {error}", path.display()))
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

#[derive(Debug, Clone, Copy)]
struct MarkerPixelRect {
  x_min: f64,
  y_min: f64,
  x_max: f64,
  y_max: f64,
}

impl MarkerPixelRect {
  fn normalized(self) -> Self {
    Self {
      x_min: self.x_min.min(self.x_max),
      y_min: self.y_min.min(self.y_max),
      x_max: self.x_min.max(self.x_max),
      y_max: self.y_min.max(self.y_max),
    }
  }
}

fn invert_crosshair(image: &mut RgbaImage, rect: MarkerPixelRect) {
  let Some(bounds) = visual_square_marker_bounds(image, rect, DEFAULT_TERMINAL_CELL_ASPECT) else {
    return;
  };
  let center_x = (bounds.x_min + bounds.x_max) / 2.0;
  let center_y = (bounds.y_min + bounds.y_max) / 2.0;
  let marker_width = (bounds.x_max - bounds.x_min).max(1.0);
  let marker_height = (bounds.y_max - bounds.y_min).max(1.0);
  let horizontal_thickness = (marker_height / 6.0).max(1.0);
  let vertical_thickness = (marker_width / 6.0).max(1.0);
  let horizontal_y_min = center_y - horizontal_thickness / 2.0;
  let horizontal_y_max = center_y + horizontal_thickness / 2.0;
  let vertical_x_min = center_x - vertical_thickness / 2.0;
  let vertical_x_max = center_x + vertical_thickness / 2.0;
  let clip_x_min = bounds.x_min.floor().max(0.0) as u32;
  let clip_y_min = bounds.y_min.floor().max(0.0) as u32;
  let clip_x_max = bounds.x_max.ceil().min(f64::from(image.width())) as u32;
  let clip_y_max = bounds.y_max.ceil().min(f64::from(image.height())) as u32;
  for y in clip_y_min..clip_y_max {
    for x in clip_x_min..clip_x_max {
      let px = f64::from(x) + 0.5;
      let py = f64::from(y) + 0.5;
      let on_horizontal = py >= horizontal_y_min && py < horizontal_y_max;
      let on_vertical = px >= vertical_x_min && px < vertical_x_max;
      if on_horizontal || on_vertical {
        let pixel = image.get_pixel_mut(x, y);
        pixel.0[0] = 255_u8.saturating_sub(pixel.0[0]);
        pixel.0[1] = 255_u8.saturating_sub(pixel.0[1]);
        pixel.0[2] = 255_u8.saturating_sub(pixel.0[2]);
      }
    }
  }
}

fn invert_outline(image: &mut RgbaImage, rect: MarkerPixelRect) {
  let bounds = rect.normalized();
  if !marker_intersects_image(bounds, image.width(), image.height()) {
    return;
  }
  let thickness = outline_thickness(bounds);
  let clip_x_min = bounds.x_min.floor().max(0.0) as u32;
  let clip_y_min = bounds.y_min.floor().max(0.0) as u32;
  let clip_x_max = bounds.x_max.ceil().min(f64::from(image.width())) as u32;
  let clip_y_max = bounds.y_max.ceil().min(f64::from(image.height())) as u32;
  if clip_x_min >= clip_x_max || clip_y_min >= clip_y_max {
    return;
  }
  let left = bounds.x_min;
  let right = bounds.x_max;
  let top = bounds.y_min;
  let bottom = bounds.y_max;
  for y in clip_y_min..clip_y_max {
    for x in clip_x_min..clip_x_max {
      let px = f64::from(x) + 0.5;
      let py = f64::from(y) + 0.5;
      let on_vertical =
        (px >= left && px < left + thickness) || (px <= right && px > right - thickness);
      let on_horizontal =
        (py >= top && py < top + thickness) || (py <= bottom && py > bottom - thickness);
      if on_vertical || on_horizontal {
        let pixel = image.get_pixel_mut(x, y);
        pixel.0[0] = 255_u8.saturating_sub(pixel.0[0]);
        pixel.0[1] = 255_u8.saturating_sub(pixel.0[1]);
        pixel.0[2] = 255_u8.saturating_sub(pixel.0[2]);
      }
    }
  }
}

fn outline_thickness(rect: MarkerPixelRect) -> f64 {
  let short_side = (rect.x_max - rect.x_min)
    .abs()
    .min((rect.y_max - rect.y_min).abs());
  (short_side / 240.0).clamp(1.0, 3.0)
}

fn visual_square_marker_bounds(
  image: &RgbaImage,
  rect: MarkerPixelRect,
  cell_aspect: f64,
) -> Option<MarkerPixelRect> {
  if !marker_intersects_image(rect, image.width(), image.height()) {
    return None;
  }

  let center_x = (rect.x_min + rect.x_max) / 2.0;
  let center_y = (rect.y_min + rect.y_max) / 2.0;
  let width = (rect.x_max - rect.x_min).max(1.0);
  let height = (rect.y_max - rect.y_min).max(1.0);
  let cell_aspect = cell_aspect.clamp(0.1, 10.0);
  let side = if cell_aspect < 1.0 {
    width.min(height * cell_aspect)
  } else {
    (width / cell_aspect).min(height)
  };
  let side = side.max(1.0);
  let bounds = MarkerPixelRect {
    x_min: center_x - side / 2.0,
    y_min: center_y - side / 2.0,
    x_max: center_x + side / 2.0,
    y_max: center_y + side / 2.0,
  };
  marker_intersects_image(bounds, image.width(), image.height()).then_some(bounds)
}

fn pdf_rect_to_source_marker_pixels(
  page: &PageImage,
  page_size: (u32, u32),
  rect: PdfRect,
  source_width: u32,
  source_height: u32,
) -> Option<MarkerPixelRect> {
  let (full_width, full_height, slice_x, slice_y) = if let Some(slice) = &page.slice {
    (
      slice.full_pixel_width.max(1),
      slice.full_pixel_height.max(1),
      slice.slice_x,
      slice.slice_y,
    )
  } else {
    (source_width.max(1), source_height.max(1), 0, 0)
  };
  let full_rect = pdf_rect_to_full_page_marker_pixels(page_size, rect, full_width, full_height)?;
  let source_rect = MarkerPixelRect {
    x_min: full_rect.x_min - f64::from(slice_x),
    y_min: full_rect.y_min - f64::from(slice_y),
    x_max: full_rect.x_max - f64::from(slice_x),
    y_max: full_rect.y_max - f64::from(slice_y),
  };
  marker_intersects_image(source_rect, source_width, source_height).then_some(source_rect)
}

fn pdf_rect_to_full_page_marker_pixels(
  page_size: (u32, u32),
  rect: PdfRect,
  pixel_width: u32,
  pixel_height: u32,
) -> Option<MarkerPixelRect> {
  let page_width = f64::from(page_size.0.max(1));
  let page_height = f64::from(page_size.1.max(1));
  let rect = rect.normalized();
  if rect.is_empty() {
    return None;
  }
  let x_scale = f64::from(pixel_width.max(1)) / page_width;
  let y_scale = f64::from(pixel_height.max(1)) / page_height;
  Some(MarkerPixelRect {
    x_min: rect.x_min * x_scale,
    y_min: rect.y_min * y_scale,
    x_max: rect.x_max * x_scale,
    y_max: rect.y_max * y_scale,
  })
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

fn pdf_rect_to_selection_crop_marker_pixels(
  selection: PdfSelection,
  rect: PdfRect,
  pixel_width: u32,
  pixel_height: u32,
) -> Option<MarkerPixelRect> {
  let selection_rect = selection.rect.normalized();
  let rect = rect.normalized();
  if rect.is_empty() {
    return None;
  }
  let selection_width = selection_rect.width().max(1.0);
  let selection_height = selection_rect.height().max(1.0);
  let x_scale = f64::from(pixel_width.max(1)) / selection_width;
  let y_scale = f64::from(pixel_height.max(1)) / selection_height;
  let marker = MarkerPixelRect {
    x_min: (rect.x_min - selection_rect.x_min) * x_scale,
    y_min: (rect.y_min - selection_rect.y_min) * y_scale,
    x_max: (rect.x_max - selection_rect.x_min) * x_scale,
    y_max: (rect.y_max - selection_rect.y_min) * y_scale,
  };
  marker_intersects_image(marker, pixel_width, pixel_height).then_some(marker)
}

fn marker_intersects_image(rect: MarkerPixelRect, width: u32, height: u32) -> bool {
  rect.x_max > 0.0
    && rect.y_max > 0.0
    && rect.x_min < f64::from(width)
    && rect.y_min < f64::from(height)
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

fn page_image_from_path(
  page_index: usize,
  path: PathBuf,
  slice: Option<crate::pdf::PageSliceMetadata>,
) -> Result<PageImage, String> {
  let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
  let (width, height) =
    image::image_dimensions(&path).map_err(|error| format!("failed to read image: {error}"))?;
  Ok(PageImage {
    page_index,
    path,
    width,
    height,
    size_bytes: metadata.len(),
    modified_nanos: metadata
      .modified()
      .ok()
      .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
      .map(|duration| duration.as_nanos())
      .unwrap_or_default(),
    slice,
  })
}

fn transformed_cache_key(
  page: &PageImage,
  page_size: (u32, u32),
  rect: PdfRect,
  label: &str,
) -> String {
  let mut hasher = base_image_hasher(page, label);
  hasher.update(page_size.0.to_le_bytes());
  hasher.update(page_size.1.to_le_bytes());
  hash_pdf_rect(&mut hasher, rect);
  hex::encode(hasher.finalize())
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

fn base_image_hasher(page: &PageImage, label: &str) -> Sha256 {
  let mut hasher = Sha256::new();
  hasher.update(b"pdf-tui-selection-v1");
  hasher.update(label.as_bytes());
  hasher.update(page.path.to_string_lossy().as_bytes());
  hasher.update(page.size_bytes.to_le_bytes());
  hasher.update(page.modified_nanos.to_le_bytes());
  hasher.update(page.width.to_le_bytes());
  hasher.update(page.height.to_le_bytes());
  if let Some(slice) = &page.slice {
    hasher.update(slice.full_pixel_width.to_le_bytes());
    hasher.update(slice.full_pixel_height.to_le_bytes());
    hasher.update(slice.slice_x.to_le_bytes());
    hasher.update(slice.slice_y.to_le_bytes());
    hasher.update(slice.slice_width.to_le_bytes());
    hasher.update(slice.slice_height.to_le_bytes());
    hasher.update(slice.cache_key.as_bytes());
  }
  hasher
}

fn transformed_crop_cache_key(
  crop: &PageImage,
  selection: PdfSelection,
  marker: PdfRect,
  label: &str,
) -> String {
  let mut hasher = base_image_hasher(crop, label);
  hasher.update(selection.page_index.to_le_bytes());
  hasher.update(selection.page_width.to_le_bytes());
  hasher.update(selection.page_height.to_le_bytes());
  hash_pdf_rect(&mut hasher, selection.rect);
  hash_pdf_rect(&mut hasher, marker);
  hex::encode(hasher.finalize())
}

fn hash_pdf_rect(hasher: &mut Sha256, rect: PdfRect) {
  hasher.update(rect.x_min.to_le_bytes());
  hasher.update(rect.y_min.to_le_bytes());
  hasher.update(rect.x_max.to_le_bytes());
  hasher.update(rect.y_max.to_le_bytes());
}
