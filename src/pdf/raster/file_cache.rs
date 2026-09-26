use std::{
  collections::HashMap,
  fs as std_fs,
  io::ErrorKind,
  path::{Path, PathBuf},
  time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use image::ImageFormat;
use tokio::fs;
use tracing::warn;

use crate::cache;

use super::super::{
  document::{PageImage, PdfDocument, modified_nanos},
  store::PageRequestKey,
};

pub(super) async fn read_cached_page_image(
  page_index: usize,
  path: &Path,
) -> Result<Option<PageImage>> {
  let metadata = match fs::metadata(path).await {
    Ok(metadata) => metadata,
    Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
    Err(error) => return Err(error).with_context(|| format!("failed to stat {}", path.display())),
  };
  let (width, height) = match image_dimensions(path).await {
    Ok(dimensions) => dimensions,
    Err(error) => {
      warn!(
        path = %path.display(),
        %error,
        "ignoring invalid pdf page image cache"
      );
      let _ = fs::remove_file(path).await;
      return Ok(None);
    }
  };
  cache::touch_cache_entry(path).await;
  Ok(Some(PageImage {
    page_index,
    path: path.to_path_buf(),
    width,
    height,
    size_bytes: metadata.len(),
    modified_nanos: modified_nanos(&metadata),
    slice: None,
  }))
}

/// Serializes rendering of one cache entry group across tasks and
/// processes. Uses the OS-locked cache locks, so a lock left behind by a
/// killed process is reclaimed instead of blocking until it ages out.
pub(super) async fn acquire_page_image_lock(output_path: &Path) -> Result<cache::CacheFileLock> {
  cache::acquire_cache_file_lock(output_path).await
}

pub(super) fn page_output_path(
  document: &PdfDocument,
  page_index: usize,
  target_width: u32,
  target_height: u32,
) -> PathBuf {
  let page_number = page_index + 1;
  document.page_cache_dir.join(format!(
    "{}-p{page_number:05}-{}x{}.png",
    document.cache_key(target_width.max(1), target_height.max(1)),
    target_width.max(1),
    target_height.max(1)
  ))
}

pub(super) fn page_batch_window(document: &PdfDocument, page_index: usize) -> (usize, usize) {
  let batch_size = document.pdf_raster_batch_pages.max(1);
  let start = (page_index / batch_size) * batch_size;
  let end = start
    .saturating_add(batch_size.saturating_sub(1))
    .min(document.page_count.saturating_sub(1));
  (start, end)
}

pub(super) fn page_batch_lock_path(
  document: &PdfDocument,
  key: PageRequestKey,
  batch_start: usize,
  batch_end: usize,
) -> PathBuf {
  document.page_cache_dir.join(format!(
    "{}-pages{:05}-{:05}-{}x{}",
    document.cache_key(key.target_width.max(1), key.target_height.max(1)),
    batch_start + 1,
    batch_end + 1,
    key.target_width.max(1),
    key.target_height.max(1)
  ))
}

pub(super) async fn image_dimensions(path: &Path) -> Result<(u32, u32)> {
  let path = path.to_path_buf();
  tokio::task::spawn_blocking(move || {
    image::image_dimensions(&path)
      .with_context(|| format!("failed to read image dimensions {}", path.display()))
  })
  .await
  .map_err(|error| anyhow::anyhow!("image dimension worker failed: {error}"))?
}

pub(super) struct TempWorkDir {
  path: PathBuf,
}

impl TempWorkDir {
  pub(super) fn path(&self) -> &Path {
    &self.path
  }

  pub(super) fn into_path(self) -> PathBuf {
    let path = self.path.clone();
    std::mem::forget(self);
    path
  }
}

impl Drop for TempWorkDir {
  fn drop(&mut self) {
    let _ = std_fs::remove_dir_all(&self.path);
  }
}

pub(super) async fn create_temp_work_dir(base: &Path, label: &str) -> Result<TempWorkDir> {
  let dir = base.join(format!("{}-{}-{}", label, std::process::id(), now_nanos()));
  fs::create_dir_all(&dir)
    .await
    .with_context(|| format!("failed to create {}", dir.display()))?;
  Ok(TempWorkDir { path: dir })
}

/// Moves a finished temporary file into the cache. Work directories live
/// under the system temp dir, often another filesystem (tmpfs) where
/// `rename` fails; the fallback copies next to the destination first so
/// readers never observe a partially written cache entry.
pub(super) async fn persist_temp_file(temp_path: &Path, output_path: &Path) -> Result<()> {
  if let Some(parent) = output_path.parent() {
    fs::create_dir_all(parent)
      .await
      .with_context(|| format!("failed to create {}", parent.display()))?;
  }
  if fs::rename(temp_path, output_path).await.is_ok() {
    return Ok(());
  }
  let copied = cache::copy_file_atomic(temp_path, output_path).await;
  let _ = fs::remove_file(temp_path).await;
  copied
}

pub(super) async fn collect_numbered_png_outputs(
  prefix: &Path,
  first_page: usize,
  last_page: usize,
) -> Result<HashMap<usize, PathBuf>> {
  let Some(parent) = prefix.parent() else {
    bail!("pdftoppm output prefix has no parent: {}", prefix.display());
  };
  let prefix_name = prefix
    .file_name()
    .and_then(|name| name.to_str())
    .unwrap_or("page");
  let mut entries = fs::read_dir(parent)
    .await
    .with_context(|| format!("failed to read {}", parent.display()))?;
  let mut outputs = HashMap::new();
  let mut unexpected = Vec::new();
  while let Some(entry) = entries
    .next_entry()
    .await
    .with_context(|| format!("failed to read {}", parent.display()))?
  {
    let file_name = entry.file_name();
    let Some(file_name) = file_name.to_str() else {
      continue;
    };
    if !file_name.ends_with(".png") {
      continue;
    }
    if !file_name.starts_with(prefix_name) {
      unexpected.push(file_name.to_string());
      continue;
    }
    let Some(number) = file_name
      .strip_prefix(prefix_name)
      .and_then(|suffix| suffix.strip_prefix('-'))
      .and_then(|suffix| suffix.strip_suffix(".png"))
      .and_then(|number| number.parse::<usize>().ok())
    else {
      unexpected.push(file_name.to_string());
      continue;
    };
    if !(first_page..=last_page).contains(&number) {
      unexpected.push(file_name.to_string());
      continue;
    }
    if outputs.insert(number, entry.path()).is_some() {
      bail!(
        "pdftoppm produced duplicate output for page {number} under {}",
        parent.display()
      );
    }
  }
  if !unexpected.is_empty() {
    unexpected.sort();
    bail!(
      "pdftoppm produced unexpected output under {}: {}",
      parent.display(),
      unexpected.join(", ")
    );
  }
  let missing = (first_page..=last_page)
    .filter(|page| !outputs.contains_key(page))
    .collect::<Vec<_>>();
  if !missing.is_empty() {
    let mut seen = outputs.keys().copied().collect::<Vec<_>>();
    seen.sort();
    bail!(
      "pdftoppm output missing pages {:?} under {} (saw pages {:?})",
      missing,
      parent.display(),
      seen
    );
  }
  Ok(outputs)
}

pub(super) fn single_png_path_for_prefix(prefix: &Path) -> PathBuf {
  prefix.with_extension("png")
}

fn now_nanos() -> u128 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_nanos()
}

/// Encodes `image` as a PNG straight into the cache via a temporary
/// sibling, so readers never see a partial file.
pub(super) async fn write_png_atomic(
  image: image::DynamicImage,
  output_path: PathBuf,
) -> Result<()> {
  if let Some(parent) = output_path.parent() {
    fs::create_dir_all(parent)
      .await
      .with_context(|| format!("failed to create {}", parent.display()))?;
  }
  tokio::task::spawn_blocking(move || {
    cache::write_file_atomic_sync(&output_path, |temp| {
      image.save_with_format(temp, ImageFormat::Png)
    })
    .map_err(anyhow::Error::msg)
  })
  .await
  .map_err(|error| anyhow::anyhow!("image writer failed: {error}"))?
}
