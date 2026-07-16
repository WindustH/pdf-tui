use std::{
  collections::HashMap,
  fs as std_fs,
  io::ErrorKind,
  path::{Path, PathBuf},
  time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use image::ImageFormat;
use tokio::{fs, io::AsyncWriteExt, process::Command, time::sleep};
use tracing::{debug, warn};

use crate::cache;

use super::{
  document::{PageImage, PageSliceMetadata, PageSliceSpec, PdfDocument, modified_nanos},
  store::PageRequestKey,
};

pub(super) async fn render_page_image(
  document: &PdfDocument,
  key: PageRequestKey,
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

  if let Some(page) = read_cached_page_image(key.page_index, &output_path).await? {
    debug!(
      page = page_number,
      path = %output_path.display(),
      "using cached pdf page image"
    );
    return Ok(page);
  }

  let (batch_start, batch_end) = page_batch_window(document, key.page_index);
  let _lock =
    acquire_page_image_lock(&page_batch_lock_path(document, key, batch_start, batch_end)).await?;
  if let Some(page) = read_cached_page_image(key.page_index, &output_path).await? {
    debug!(
      page = page_number,
      path = %output_path.display(),
      "using cached pdf page image after waiting for lock"
    );
    return Ok(page);
  }

  render_missing_page_batch(document, key, batch_start, batch_end).await?;

  read_cached_page_image(key.page_index, &output_path)
    .await?
    .with_context(|| format!("failed to read rendered page {}", output_path.display()))
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

  let temp_dir = create_temp_work_dir(&document.page_temp_dir, "pdftoppm").await?;
  let temp_prefix = temp_dir.path().join("page");
  debug!(
    first_page = batch_start + 1,
    last_page = batch_end + 1,
    missing = missing.len(),
    target_width,
    target_height,
    temp = %temp_prefix.display(),
    "rendering pdf page batch with pdftoppm"
  );
  let temp_outputs = if batch_start == batch_end {
    let page_number = batch_start + 1;
    let temp_prefix = temp_dir.path().join(format!("page-{page_number:05}"));
    let temp_output = single_png_path_for_prefix(&temp_prefix);
    run_pdftoppm_single(
      document,
      page_number,
      target_width,
      target_height,
      &temp_prefix,
    )
    .await?;
    HashMap::from([(page_number, temp_output)])
  } else {
    run_pdftoppm_batch(
      document,
      batch_start + 1,
      batch_end + 1,
      target_width,
      target_height,
      &temp_prefix,
    )
    .await?;
    collect_numbered_png_outputs(&temp_prefix, batch_start + 1, batch_end + 1).await?
  };

  for index in missing {
    let page_number = index + 1;
    let temp_output_path = temp_outputs.get(&page_number).with_context(|| {
      format!(
        "pdftoppm did not produce page {page_number} under {}",
        temp_dir.path().display()
      )
    })?;
    image_dimensions(&temp_output_path)
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

async fn run_pdftoppm_batch(
  document: &PdfDocument,
  first_page: usize,
  last_page: usize,
  target_width: u32,
  target_height: u32,
  temp_prefix: &Path,
) -> Result<()> {
  let output = Command::new(&document.pdftoppm_bin)
    .arg("-f")
    .arg(first_page.to_string())
    .arg("-l")
    .arg(last_page.to_string())
    .arg("-scale-to-x")
    .arg(target_width.to_string())
    .arg("-scale-to-y")
    .arg(target_height.to_string())
    .arg("-png")
    .arg(&document.path)
    .arg(temp_prefix)
    .output()
    .await
    .with_context(|| {
      format!(
        "failed to run {}; install poppler-utils",
        document.pdftoppm_bin
      )
    })?;
  if !output.status.success() {
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!(
      "pdftoppm failed for pages {first_page}-{last_page}: {}",
      stderr.trim()
    );
  }
  Ok(())
}

async fn run_pdftoppm_single(
  document: &PdfDocument,
  page_number: usize,
  target_width: u32,
  target_height: u32,
  temp_prefix: &Path,
) -> Result<()> {
  let output = Command::new(&document.pdftoppm_bin)
    .arg("-f")
    .arg(page_number.to_string())
    .arg("-l")
    .arg(page_number.to_string())
    .arg("-singlefile")
    .arg("-scale-to-x")
    .arg(target_width.to_string())
    .arg("-scale-to-y")
    .arg(target_height.to_string())
    .arg("-png")
    .arg(&document.path)
    .arg(temp_prefix)
    .output()
    .await
    .with_context(|| {
      format!(
        "failed to run {}; install poppler-utils",
        document.pdftoppm_bin
      )
    })?;
  if !output.status.success() {
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!("pdftoppm failed for page {page_number}: {}", stderr.trim());
  }
  Ok(())
}

async fn read_cached_page_image(page_index: usize, path: &Path) -> Result<Option<PageImage>> {
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

struct PageImageLock {
  path: PathBuf,
}

impl Drop for PageImageLock {
  fn drop(&mut self) {
    let _ = std_fs::remove_file(&self.path);
  }
}

async fn acquire_page_image_lock(output_path: &Path) -> Result<PageImageLock> {
  let lock_path = lock_path_for(output_path);
  loop {
    match fs::OpenOptions::new()
      .write(true)
      .create_new(true)
      .open(&lock_path)
      .await
    {
      Ok(mut file) => {
        let _ = file
          .write_all(format!("pid={}\n", std::process::id()).as_bytes())
          .await;
        return Ok(PageImageLock { path: lock_path });
      }
      Err(error) if error.kind() == ErrorKind::AlreadyExists => {
        if lock_is_stale(&lock_path).await {
          warn!(
            lock = %lock_path.display(),
            "removing stale pdf page image cache lock"
          );
          let _ = fs::remove_file(&lock_path).await;
          continue;
        }
        sleep(Duration::from_millis(40)).await;
      }
      Err(error) => {
        return Err(error)
          .with_context(|| format!("failed to create cache lock {}", lock_path.display()));
      }
    }
  }
}

fn lock_path_for(path: &Path) -> PathBuf {
  let mut name = path
    .file_name()
    .map(|name| name.to_os_string())
    .unwrap_or_else(|| "page".into());
  name.push(".lock");
  path.with_file_name(name)
}

async fn lock_is_stale(path: &Path) -> bool {
  fs::metadata(path)
    .await
    .ok()
    .and_then(|metadata| metadata.modified().ok())
    .and_then(|modified| modified.elapsed().ok())
    .is_some_and(|age| age > Duration::from_secs(600))
}

fn page_output_path(
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

fn page_batch_window(document: &PdfDocument, page_index: usize) -> (usize, usize) {
  let batch_size = document.pdftoppm_batch_pages.max(1);
  let start = (page_index / batch_size) * batch_size;
  let end = start
    .saturating_add(batch_size.saturating_sub(1))
    .min(document.page_count.saturating_sub(1));
  (start, end)
}

fn page_batch_lock_path(
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

async fn image_dimensions(path: &Path) -> Result<(u32, u32)> {
  let path = path.to_path_buf();
  tokio::task::spawn_blocking(move || {
    image::image_dimensions(&path)
      .with_context(|| format!("failed to read image dimensions {}", path.display()))
  })
  .await
  .map_err(|error| anyhow::anyhow!("image dimension worker failed: {error}"))?
}

struct TempWorkDir {
  path: PathBuf,
}

impl TempWorkDir {
  fn path(&self) -> &Path {
    &self.path
  }
}

impl Drop for TempWorkDir {
  fn drop(&mut self) {
    let _ = std_fs::remove_dir_all(&self.path);
  }
}

async fn create_temp_work_dir(base: &Path, label: &str) -> Result<TempWorkDir> {
  let dir = base.join(format!("{}-{}-{}", label, std::process::id(), now_nanos()));
  fs::create_dir_all(&dir)
    .await
    .with_context(|| format!("failed to create {}", dir.display()))?;
  Ok(TempWorkDir { path: dir })
}

async fn persist_temp_file(temp_path: &Path, output_path: &Path) -> Result<()> {
  if let Some(parent) = output_path.parent() {
    fs::create_dir_all(parent)
      .await
      .with_context(|| format!("failed to create {}", parent.display()))?;
  }
  match fs::rename(temp_path, output_path).await {
    Ok(()) => Ok(()),
    Err(error) => {
      fs::copy(temp_path, output_path).await.with_context(|| {
        format!(
          "failed to copy {} to {} after rename failed ({error})",
          temp_path.display(),
          output_path.display()
        )
      })?;
      let _ = fs::remove_file(temp_path).await;
      Ok(())
    }
  }
}

async fn collect_numbered_png_outputs(
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

fn single_png_path_for_prefix(prefix: &Path) -> PathBuf {
  prefix.with_extension("png")
}

fn now_nanos() -> u128 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_nanos()
}

pub(super) async fn render_page_slice_image(
  document: &PdfDocument,
  spec: PageSliceSpec,
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

  let full_page = render_page_image(
    document,
    PageRequestKey {
      page_index: spec.page_index,
      target_width: spec.target_width,
      target_height: spec.target_height,
    },
  )
  .await?;
  let cache_key = document.slice_cache_key(spec);
  let page_number = spec.page_index + 1;
  let output_path = page_slice_output_path(document, spec, &cache_key);
  let metadata_path = output_path.with_extension("toml");

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
    viewport_width: spec.viewport_width,
    viewport_height: spec.viewport_height,
    scroll_divisor: spec.scroll_divisor,
    cache_key: cache_key.clone(),
  };

  if !output_path.exists() {
    let _lock = acquire_page_image_lock(&page_slice_group_lock_path(document, spec)).await?;
    if !output_path.exists() {
      render_missing_page_slice_group(document, &full_page, spec).await?;
    }
  }

  if output_path.exists() {
    debug!(
      page = page_number,
      slice = spec.slice_index + 1,
      slice_count = spec.slice_count,
      path = %output_path.display(),
      "using cached pdf page slice image"
    );
  }

  write_slice_metadata(&metadata_path, &metadata).await?;
  cache::touch_cache_entry(&output_path).await;
  cache::touch_cache_entry(&metadata_path).await;

  let (width, height) = image_dimensions(&output_path)
    .await
    .with_context(|| format!("failed to read {}", output_path.display()))?;
  let file_metadata = fs::metadata(&output_path)
    .await
    .with_context(|| format!("failed to stat {}", output_path.display()))?;
  Ok(PageImage {
    page_index: spec.page_index,
    path: output_path,
    width,
    height,
    size_bytes: file_metadata.len(),
    modified_nanos: modified_nanos(&file_metadata),
    slice: Some(metadata),
  })
}

async fn write_slice_metadata(path: &Path, metadata: &PageSliceMetadata) -> Result<()> {
  let encoded = toml::to_string_pretty(metadata).context("failed to encode slice metadata")?;
  fs::write(path, encoded)
    .await
    .with_context(|| format!("failed to write {}", path.display()))
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
      !page_slice_output_path(document, *sibling, &cache_key).exists()
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
    if output_path.exists() {
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
    let slice = full_image.crop_imm(0, slice_y, full_width, slice_height);
    let temp_path = temp_output_path_for(&document.page_temp_dir, &output_path);
    write_png_atomic(slice, temp_path.clone(), output_path.clone()).await?;
    let metadata = page_slice_metadata(
      document,
      sibling,
      full_width,
      full_height,
      slice_y,
      slice_height,
      cache_key,
    );
    write_slice_metadata(&output_path.with_extension("toml"), &metadata).await?;
    cache::touch_cache_entry(&output_path).await;
    cache::touch_cache_entry(&output_path.with_extension("toml")).await;
  }
  Ok(())
}

fn page_slice_metadata(
  document: &PdfDocument,
  spec: PageSliceSpec,
  full_width: u32,
  full_height: u32,
  slice_y: u32,
  slice_height: u32,
  cache_key: String,
) -> PageSliceMetadata {
  PageSliceMetadata {
    source_pdf: document.path.to_string_lossy().into_owned(),
    source_size_bytes: document.size_bytes,
    source_modified_nanos: document.modified_nanos.to_string(),
    page_index: spec.page_index,
    page_number: spec.page_index + 1,
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
    viewport_width: spec.viewport_width,
    viewport_height: spec.viewport_height,
    scroll_divisor: spec.scroll_divisor,
    cache_key,
  }
}

fn sibling_slice_specs(spec: PageSliceSpec) -> Vec<PageSliceSpec> {
  let spec = spec.normalized();
  let cell_pixel_height = spec
    .target_height
    .checked_div(u32::from(spec.full_cell_height.max(1)))
    .unwrap_or(1)
    .max(1);
  (0..spec.slice_count)
    .map(|slice_index| {
      let slice_cell_start =
        (u64::from(spec.full_cell_height) * u64::from(slice_index)) / u64::from(spec.slice_count);
      let slice_cell_end = (u64::from(spec.full_cell_height)
        * u64::from(slice_index.saturating_add(1)))
        / u64::from(spec.slice_count);
      let slice_y = slice_cell_start
        .saturating_mul(u64::from(cell_pixel_height))
        .min(u64::from(u32::MAX)) as u32;
      let slice_height = slice_cell_end
        .saturating_sub(slice_cell_start)
        .saturating_mul(u64::from(cell_pixel_height))
        .max(1)
        .min(u64::from(u32::MAX)) as u32;
      PageSliceSpec {
        slice_index,
        slice_y,
        slice_height,
        ..spec
      }
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

async fn write_png_atomic(
  image: image::DynamicImage,
  temp_path: PathBuf,
  output_path: PathBuf,
) -> Result<()> {
  if let Some(parent) = temp_path.parent() {
    fs::create_dir_all(parent)
      .await
      .with_context(|| format!("failed to create {}", parent.display()))?;
  }
  let _ = fs::remove_file(&temp_path).await;
  let write_path = temp_path.clone();
  tokio::task::spawn_blocking(move || {
    image
      .save_with_format(&write_path, ImageFormat::Png)
      .with_context(|| format!("failed to write {}", write_path.display()))
  })
  .await
  .map_err(|error| anyhow::anyhow!("image writer failed: {error}"))??;
  persist_temp_file(&temp_path, &output_path).await
}

fn temp_output_path_for(temp_dir: &Path, output_path: &Path) -> PathBuf {
  let nanos = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_nanos();
  let mut name = output_path
    .file_name()
    .map(|name| name.to_os_string())
    .unwrap_or_else(|| "slice.png".into());
  name.push(format!(".tmp-{}-{nanos}", std::process::id()));
  temp_dir.join(name)
}
