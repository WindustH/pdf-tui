//! Pure page-coordinate geometry shared by rasterization and selection tools.
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
    };
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

/// Bound selection intermediates to 16 megapixels (64 MiB of RGBA) and
/// 8192 pixels per edge, even when a tiny crop requests a very large scale.
/// Backends may allocate additional working buffers beyond this bitmap.
pub(crate) fn bounded_selection_page_size(width: f64, height: f64) -> (u32, u32) {
  if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
    return (1, 1);
  }
  let scale = (16_777_216.0 / (width * height))
    .sqrt()
    .min(8192.0 / width)
    .min(8192.0 / height)
    .min(1.0);
  (
    (width * scale).floor().max(1.0) as u32,
    (height * scale).floor().max(1.0) as u32,
  )
}
