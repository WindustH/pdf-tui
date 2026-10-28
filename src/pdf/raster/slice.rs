use std::path::PathBuf;

use anyhow::{Context, Result};
use tokio::fs;
use tracing::debug;

use crate::layout::grid_slice_span;

use super::super::{
  document::{PageImage, PageSliceMetadata, PageSliceSpec, PdfDocument},
  store::PageRequestKey,
};
use super::{
  file_cache::{acquire_page_image_lock, read_cached_page_image, write_png_atomic},
  render_page_image_with_batch_mode,
};

pub(in crate::pdf) async fn render_page_slice_image(
  document: &PdfDocument,
  spec: PageSliceSpec,
) -> Result<PageImage> {
  render_page_slice_image_with_batch_mode(document, spec, false).await
}

pub(in crate::pdf) async fn preload_page_slice_image(
  document: &PdfDocument,
  spec: PageSliceSpec,
) -> Result<PageImage> {
  render_page_slice_image_with_batch_mode(document, spec, true).await
}

async fn render_page_slice_image_with_batch_mode(
  document: &PdfDocument,
  spec: PageSliceSpec,
  ensure_full_batch: bool,
) -> Result<PageImage> {
  let spec = spec.normalized();
  fs::create_dir_all(&document.page_cache_dir)
    .await
    .with_context(|| {
      format!(
        "failed to create page cache {}",
        document.page_cache_dir.display()
      )
    })?;

  let full_page = render_page_image_with_batch_mode(
    document,
    PageRequestKey {
      page_index: spec.page_index,
      target_width: spec.target_width,
      target_height: spec.target_height,
    },
    ensure_full_batch,
  )
  .await?;
  let cache_key = document.slice_cache_key(spec);
  let page_number = spec.page_index + 1;
  let output_path = page_slice_output_path(document, spec, &cache_key);

  let full_width = full_page.width.max(1);
  let full_height = full_page.height.max(1);
  let slice_y = spec.slice_y.min(full_height.saturating_sub(1));
  let slice_height = spec
    .slice_height
    .min(full_height.saturating_sub(slice_y))
    .max(1);
  let metadata = PageSliceMetadata {
    source_pdf: document.path.to_string_lossy().into_owned(),
    source_size_bytes: document.size_bytes,
    source_modified_nanos: document.modified_nanos.to_string(),
    page_index: spec.page_index,
    page_number,
    slice_index: spec.slice_index,
    slice_count: spec.slice_count,
    full_pixel_width: full_width,
    full_pixel_height: full_height,
    slice_x: 0,
    slice_y,
    slice_width: full_width,
    slice_height,
    cell_width: spec.cell_width,
    cell_height: spec.cell_height,
    full_cell_width: spec.full_cell_width,
    full_cell_height: spec.full_cell_height,
    grid_cell_height: spec.grid_cell_height,
    viewport_width: spec.viewport_width,
    viewport_height: spec.viewport_height,
    scroll_divisor: spec.scroll_divisor,
    cache_key: cache_key.clone(),
  };

  let mut image = read_cached_page_image(spec.page_index, &output_path).await?;
  if image.is_none() {
    let _lock = acquire_page_image_lock(&page_slice_group_lock_path(document, spec)).await?;
    image = read_cached_page_image(spec.page_index, &output_path).await?;
    if image.is_none() {
      render_missing_page_slice_group(document, &full_page, spec).await?;
      image = read_cached_page_image(spec.page_index, &output_path).await?;
    }
  }
  let mut image = image.context("slice regeneration produced no readable image")?;
  image.slice = Some(metadata);
  Ok(image)
}

async fn render_missing_page_slice_group(
  document: &PdfDocument,
  full_page: &PageImage,
  spec: PageSliceSpec,
) -> Result<()> {
  let specs = sibling_slice_specs(spec)
    .into_iter()
    .filter(|sibling| {
      let cache_key = document.slice_cache_key(*sibling);
      !img_tui::cache::paths::contains(&page_slice_output_path(document, *sibling, &cache_key))
    })
    .collect::<Vec<_>>();
  if specs.is_empty() {
    return Ok(());
  }

  let page_number = spec.page_index + 1;
  debug!(
    page = page_number,
    slice_count = spec.slice_count,
    missing = specs.len(),
    source = %full_page.path.display(),
    "rendering pdf page slice group"
  );
  let full_image_path = full_page.path.clone();
  let full_image = tokio::task::spawn_blocking(move || {
    image::open(&full_image_path)
      .with_context(|| format!("failed to open {}", full_image_path.display()))
  })
  .await
  .map_err(|error| anyhow::anyhow!("image worker failed: {error}"))??;
  let full_width = full_page.width.max(1);
  let full_height = full_page.height.max(1);
  for sibling in specs {
    let cache_key = document.slice_cache_key(sibling);
    let output_path = page_slice_output_path(document, sibling, &cache_key);
    if img_tui::cache::paths::contains(&output_path) {
      continue;
    }
    let slice_y = sibling.slice_y.min(full_height.saturating_sub(1));
    let slice_height = sibling
      .slice_height
      .min(full_height.saturating_sub(slice_y))
      .max(1);
    debug!(
      page = page_number,
      slice = sibling.slice_index + 1,
      slice_count = sibling.slice_count,
      path = %output_path.display(),
      slice_y,
      slice_height,
      "writing pdf page slice image"
    );
    let slice = img_tui::slicing::PixelRegion {
      x: 0,
      y: slice_y,
      width: full_width,
      height: slice_height,
    }
    .crop(&full_image)?;
    write_png_atomic(slice, output_path.clone()).await?;
  }
  Ok(())
}

fn sibling_slice_specs(spec: PageSliceSpec) -> Vec<PageSliceSpec> {
  let spec = spec.normalized();
  let cell_pixel_height = spec
    .target_height
    .checked_div(u32::from(spec.full_cell_height.max(1)))
    .unwrap_or(1)
    .max(1);
  (0..spec.slice_count)
    .filter_map(|slice_index| {
      let (slice_cell_y, slice_cell_height) = grid_slice_span(
        spec.grid_cell_height,
        spec.slice_count,
        slice_index,
        spec.full_cell_height,
      );
      if slice_cell_height == 0 {
        return None;
      }
      let slice_y = u64::from(slice_cell_y)
        .saturating_mul(u64::from(cell_pixel_height))
        .min(u64::from(u32::MAX)) as u32;
      let slice_height = u64::from(slice_cell_height)
        .saturating_mul(u64::from(cell_pixel_height))
        .max(1)
        .min(u64::from(u32::MAX)) as u32;
      Some(PageSliceSpec {
        slice_index,
        slice_y,
        slice_height,
        ..spec
      })
    })
    .collect()
}

fn page_slice_output_path(document: &PdfDocument, spec: PageSliceSpec, cache_key: &str) -> PathBuf {
  let page_number = spec.page_index + 1;
  document.page_cache_dir.join(format!(
    "{}-p{page_number:05}-slice{:03}of{:03}-{}x{}-y{}h{}.png",
    cache_key,
    spec.slice_index + 1,
    spec.slice_count,
    spec.target_width,
    spec.target_height,
    spec.slice_y,
    spec.slice_height
  ))
}

fn page_slice_group_lock_path(document: &PdfDocument, spec: PageSliceSpec) -> PathBuf {
  let page_number = spec.page_index + 1;
  document.page_cache_dir.join(format!(
    "{}-p{page_number:05}-slices{:03}-{}x{}-{}h{}",
    document.cache_key(spec.target_width, spec.target_height),
    spec.slice_count,
    spec.target_width,
    spec.target_height,
    spec.full_cell_width,
    spec.full_cell_height
  ))
}
