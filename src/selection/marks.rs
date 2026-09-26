//! Selection marks drawn into images: anchor crosshairs and the outline
//! of the selection rectangle, on a page (or slice) image or on a crop of
//! an earlier selection. Results are cached as PNGs under `selection/`.

use std::{fs, path::Path};

use image::{GenericImageView, ImageFormat, RgbaImage};
use sha2::{Digest, Sha256};

use crate::{cache, pdf::PageImage};

use super::{PdfRect, PdfSelection, hash_pdf_rect, page_image_from_path};

/// Terminal cells are about twice as tall as wide; crosshairs are squared
/// up for that so they look square on screen.
const DEFAULT_TERMINAL_CELL_ASPECT: f64 = 0.5;

/// Whether `rect` (in page points) overlaps the region of the page that
/// `page` shows, so marking it would change any pixel.
pub fn page_rect_visible(page: &PageImage, page_size: (u32, u32), rect: PdfRect) -> bool {
  pdf_rect_to_source_marker_pixels(page, page_size, rect, page.width, page.height).is_some()
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
    let _lock = cache::acquire_cache_file_lock_sync(&path).map_err(|error| error.to_string())?;
    if path.exists() {
      cache::touch_cache_entry_sync(&path);
      return page_image_from_path(page.page_index, path, page.slice.clone());
    }
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
    let _lock = cache::acquire_cache_file_lock_sync(&path).map_err(|error| error.to_string())?;
    if path.exists() {
      cache::touch_cache_entry_sync(&path);
      return page_image_from_path(crop.page_index, path, None);
    }
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
  cache::write_file_atomic_sync(path, |temp| image.save_with_format(temp, ImageFormat::Png))
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
  cache::write_file_atomic_sync(path, |temp| image.save_with_format(temp, ImageFormat::Png))
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
