mod backend;
mod crop;
mod file_cache;
mod slice;

use anyhow::{Context, Result, bail};
use tokio::fs;
use tracing::debug;

use crate::{cache, config::PdfRasterBackend, selection::PdfSelection};

use super::{
  document::{PageImage, PdfDocument, TempPageImage, modified_nanos},
  store::PageRequestKey,
};
use backend::{
  render_pdf_page_batch, render_pdfium_selection_image, render_poppler_selection_image,
};
use crop::selection_crop_plan;
use file_cache::{
  acquire_page_image_lock, create_temp_work_dir, image_dimensions, page_batch_lock_path,
  page_batch_window, page_output_path, persist_temp_file, read_cached_page_image,
};
pub(super) use slice::{preload_page_slice_image, render_page_slice_image};

pub(super) async fn render_page_image(
  document: &PdfDocument,
  key: PageRequestKey,
) -> Result<PageImage> {
  render_page_image_with_batch_mode(document, key, false).await
}

pub(super) async fn preload_page_image(
  document: &PdfDocument,
  key: PageRequestKey,
) -> Result<PageImage> {
  render_page_image_with_batch_mode(document, key, true).await
}

pub(super) async fn render_uncached_page_image(
  document: &PdfDocument,
  page_index: usize,
  target_width: u32,
  target_height: u32,
) -> Result<TempPageImage> {
  let target_width = target_width.max(1);
  let target_height = target_height.max(1);
  let page_number = page_index + 1;
  let temp_dir = create_temp_work_dir(
    &document.page_temp_dir,
    &format!("{}-selection", document.raster_backend.label()),
  )
  .await?;
  let temp_prefix = temp_dir.path().join("page");
  debug!(
    page = page_number,
    target_width,
    target_height,
    backend = document.raster_backend.label(),
    temp = %temp_prefix.display(),
    "rendering uncached pdf page"
  );
  let outputs = render_pdf_page_batch(
    document,
    &[page_index],
    page_number,
    page_number,
    target_width,
    target_height,
    &temp_dir,
    &temp_prefix,
  )
  .await?;
  let output_path = outputs.get(&page_number).with_context(|| {
    format!(
      "{} did not produce page {page_number} under {}",
      document.raster_backend.label(),
      temp_dir.path().display()
    )
  })?;
  let metadata = fs::metadata(output_path)
    .await
    .with_context(|| format!("failed to stat {}", output_path.display()))?;
  let (width, height) = image_dimensions(output_path).await?;
  let temp_path = temp_dir.into_path();
  Ok(TempPageImage::new(
    PageImage {
      page_index,
      path: output_path.to_path_buf(),
      width,
      height,
      size_bytes: metadata.len(),
      modified_nanos: modified_nanos(&metadata),
      slice: None,
    },
    temp_path,
  ))
}

pub(super) async fn render_uncached_selection_image(
  document: &PdfDocument,
  selection: PdfSelection,
  crop_width: u32,
  crop_height: u32,
) -> Result<TempPageImage> {
  let crop_width = crop_width.max(1);
  let crop_height = crop_height.max(1);
  let plan = selection_crop_plan(selection, crop_width, crop_height)
    .context("selection is outside rendered page")?;
  let page_number = selection.page_index + 1;
  if matches!(document.raster_backend, PdfRasterBackend::Mutool) {
    bail!("mutool draw does not expose a reliable selection crop option");
  }
  let temp_dir = create_temp_work_dir(
    &document.page_temp_dir,
    &format!("{}-selection-crop", document.raster_backend.label()),
  )
  .await?;
  debug!(
    page = page_number,
    target_width = plan.page_width,
    target_height = plan.page_height,
    crop_x = plan.crop.x,
    crop_y = plan.crop.y,
    crop_width = plan.crop.width,
    crop_height = plan.crop.height,
    backend = document.raster_backend.label(),
    temp = %temp_dir.path().display(),
    "rendering uncached pdf selection"
  );
  let output_path = match document.raster_backend {
    PdfRasterBackend::Poppler => {
      render_poppler_selection_image(document, page_number, plan, &temp_dir).await?
    }
    PdfRasterBackend::Pdfium => {
      render_pdfium_selection_image(document, page_number, plan, temp_dir.path()).await?
    }
    PdfRasterBackend::Mutool => unreachable!("checked above"),
  };
  let metadata = fs::metadata(&output_path)
    .await
    .with_context(|| format!("failed to stat {}", output_path.display()))?;
  let (width, height) = image_dimensions(&output_path).await?;
  let temp_path = temp_dir.into_path();
  Ok(TempPageImage::new(
    PageImage {
      page_index: selection.page_index,
      path: output_path,
      width,
      height,
      size_bytes: metadata.len(),
      modified_nanos: modified_nanos(&metadata),
      slice: None,
    },
    temp_path,
  ))
}

async fn render_page_image_with_batch_mode(
  document: &PdfDocument,
  key: PageRequestKey,
  ensure_full_batch: bool,
) -> Result<PageImage> {
  fs::create_dir_all(&document.page_cache_dir)
    .await
    .with_context(|| {
      format!(
        "failed to create page cache {}",
        document.page_cache_dir.display()
      )
    })?;

  let page_number = key.page_index + 1;
  let target_width = key.target_width.max(1);
  let target_height = key.target_height.max(1);
  let output_path = page_output_path(document, key.page_index, target_width, target_height);

  if !ensure_full_batch
    && let Some(page) = read_cached_page_image(key.page_index, &output_path).await?
  {
    debug!(
      page = page_number,
      path = %output_path.display(),
      "using cached pdf page image"
    );
    return Ok(page);
  }

  ensure_page_batch_rendered(document, key).await?;

  if let Some(page) = read_cached_page_image(key.page_index, &output_path).await? {
    debug!(
      page = page_number,
      path = %output_path.display(),
      ensured_batch = ensure_full_batch,
      "using cached pdf page image after ensuring batch"
    );
    return Ok(page);
  }

  read_cached_page_image(key.page_index, &output_path)
    .await?
    .with_context(|| format!("failed to read rendered page {}", output_path.display()))
}

async fn ensure_page_batch_rendered(document: &PdfDocument, key: PageRequestKey) -> Result<()> {
  let (batch_start, batch_end) = page_batch_window(document, key.page_index);
  let _lock =
    acquire_page_image_lock(&page_batch_lock_path(document, key, batch_start, batch_end)).await?;
  render_missing_page_batch(document, key, batch_start, batch_end).await
}

async fn render_missing_page_batch(
  document: &PdfDocument,
  key: PageRequestKey,
  batch_start: usize,
  batch_end: usize,
) -> Result<()> {
  let target_width = key.target_width.max(1);
  let target_height = key.target_height.max(1);
  let mut missing = Vec::new();
  for index in batch_start..=batch_end {
    let output_path = page_output_path(document, index, target_width, target_height);
    if read_cached_page_image(index, &output_path).await?.is_none() {
      missing.push(index);
    }
  }
  if missing.is_empty() {
    return Ok(());
  }

  let temp_dir =
    create_temp_work_dir(&document.page_temp_dir, document.raster_backend.label()).await?;
  let temp_prefix = temp_dir.path().join("page");
  debug!(
    first_page = batch_start + 1,
    last_page = batch_end + 1,
    missing = missing.len(),
    target_width,
    target_height,
    backend = document.raster_backend.label(),
    temp = %temp_prefix.display(),
    "rendering pdf page batch"
  );
  let temp_outputs = render_pdf_page_batch(
    document,
    &missing,
    batch_start + 1,
    batch_end + 1,
    target_width,
    target_height,
    &temp_dir,
    &temp_prefix,
  )
  .await?;

  for index in missing {
    let page_number = index + 1;
    let temp_output_path = temp_outputs.get(&page_number).with_context(|| {
      format!(
        "{} did not produce page {page_number} under {}",
        document.raster_backend.label(),
        temp_dir.path().display()
      )
    })?;
    image_dimensions(temp_output_path)
      .await
      .with_context(|| format!("failed to read {}", temp_output_path.display()))?;
    let output_path = page_output_path(document, index, target_width, target_height);
    if read_cached_page_image(index, &output_path).await?.is_some() {
      let _ = fs::remove_file(temp_output_path).await;
      continue;
    }
    persist_temp_file(temp_output_path, &output_path).await?;
    cache::touch_cache_entry(&output_path).await;
  }
  let _ = fs::remove_dir_all(temp_dir.path()).await;
  Ok(())
}
