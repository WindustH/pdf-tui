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

pub use crate::pdf::types::{PdfPoint, PdfRect, PdfSelection, SelectionAnchor};

fn page_image_from_path(
  page_index: usize,
  path: PathBuf,
  slice: Option<crate::pdf::PageSliceMetadata>,
) -> Result<PageImage, String> {
  let path = img_tui::cache::paths::materialize(&path).map_err(|e| e.to_string())?;
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
