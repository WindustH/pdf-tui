mod document;
mod raster;
mod store;

pub use document::{PageImage, PageSliceMetadata, PageSliceSpec, PdfDocument, TempPageImage};
pub use store::PageStore;

pub async fn render_uncached_page_image_at(
  document: &PdfDocument,
  page_index: usize,
  target_width: u32,
  target_height: u32,
) -> anyhow::Result<TempPageImage> {
  raster::render_uncached_page_image(
    document,
    page_index,
    target_width.max(1),
    target_height.max(1),
  )
  .await
}

pub async fn render_uncached_selection_image_at(
  document: &PdfDocument,
  selection: crate::selection::PdfSelection,
  target_width: u32,
  target_height: u32,
) -> anyhow::Result<TempPageImage> {
  raster::render_uncached_selection_image(
    document,
    selection,
    target_width.max(1),
    target_height.max(1),
  )
  .await
}
