//! Rectangular selections in PDF page coordinates, the marks that show
//! them, and rendering selections as images.

mod crop;
mod marks;

use std::{fs, path::PathBuf, time::UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::pdf::PageImage;

pub use crop::{
  render_selection_copy_image, render_selection_preview_image, selection_image_cache_key,
  selection_preview_page_target,
};
pub use marks::{
  marker_page_image, marker_selection_crop_image, outline_page_image, outline_selection_crop_image,
  page_rect_visible,
};

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

fn hash_pdf_rect(hasher: &mut Sha256, rect: PdfRect) {
  hasher.update(rect.x_min.to_le_bytes());
  hasher.update(rect.y_min.to_le_bytes());
  hasher.update(rect.x_max.to_le_bytes());
  hasher.update(rect.y_max.to_le_bytes());
}
