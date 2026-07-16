use std::{
  fs::{self, OpenOptions},
  io::{ErrorKind, Write},
  path::{Path, PathBuf},
  process::Command,
  thread,
  time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use tracing::{debug, warn};

use crate::cache;

use super::{
  document::{PageImage, PageSliceMetadata, PageSliceSpec, PdfDocument, modified_nanos},
  store::PageRequestKey,
};

pub(super) fn render_page_image(document: &PdfDocument, key: PageRequestKey) -> Result<PageImage> {
  fs::create_dir_all(&document.page_cache_dir).with_context(|| {
    format!(
      "failed to create page cache {}",
      document.page_cache_dir.display()
    )
  })?;

  let page_number = key.page_index + 1;
  let target_width = key.target_width.max(1);
  let target_height = key.target_height.max(1);
  let prefix = document.page_cache_dir.join(format!(
    "{}-p{page_number:05}-{}x{}",
    document.cache_key(target_width, target_height),
    target_width,
    target_height
  ));
  let output_path = png_path_for_prefix(&prefix);

  if let Some(page) = read_cached_page_image(key.page_index, &output_path)? {
    debug!(
      page = page_number,
      path = %output_path.display(),
      "using cached pdf page image"
    );
    return Ok(page);
  }

  let _lock = acquire_page_image_lock(&output_path)?;
  if let Some(page) = read_cached_page_image(key.page_index, &output_path)? {
    debug!(
      page = page_number,
      path = %output_path.display(),
      "using cached pdf page image after waiting for lock"
    );
    return Ok(page);
  }
  if output_path.exists() {
    warn!(
      page = page_number,
      path = %output_path.display(),
      "removing invalid pdf page image cache"
    );
    let _ = fs::remove_file(&output_path);
  }

  let temp_prefix = temp_page_prefix(&prefix);
  let temp_output_path = png_path_for_prefix(&temp_prefix);
  let _ = fs::remove_file(&temp_output_path);
  debug!(
    page = page_number,
    path = %output_path.display(),
    temp = %temp_output_path.display(),
    "rendering pdf page image with pdftoppm"
  );
  let mut command = Command::new(&document.pdftoppm_bin);
  command
    .arg("-f")
    .arg(page_number.to_string())
    .arg("-l")
    .arg(page_number.to_string());
  command
    .arg("-scale-to-x")
    .arg(target_width.to_string())
    .arg("-scale-to-y")
    .arg(target_height.to_string());
  command
    .arg("-png")
    .arg("-singlefile")
    .arg(&document.path)
    .arg(&temp_prefix);
  let output = command.output().with_context(|| {
    format!(
      "failed to run {}; install poppler-utils",
      document.pdftoppm_bin
    )
  })?;
  if !output.status.success() {
    let _ = fs::remove_file(&temp_output_path);
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!("pdftoppm failed for page {page_number}: {}", stderr.trim());
  }
  image::image_dimensions(&temp_output_path)
    .with_context(|| format!("failed to read {}", temp_output_path.display()))?;
  fs::rename(&temp_output_path, &output_path).with_context(|| {
    format!(
      "failed to move {} to {}",
      temp_output_path.display(),
      output_path.display()
    )
  })?;

  read_cached_page_image(key.page_index, &output_path)?
    .with_context(|| format!("failed to read rendered page {}", output_path.display()))
}

fn read_cached_page_image(page_index: usize, path: &Path) -> Result<Option<PageImage>> {
  if !path.exists() {
    return Ok(None);
  }
  let (width, height) = match image::image_dimensions(path) {
    Ok(dimensions) => dimensions,
    Err(error) => {
      warn!(
        path = %path.display(),
        %error,
        "ignoring invalid pdf page image cache"
      );
      return Ok(None);
    }
  };
  let metadata =
    fs::metadata(path).with_context(|| format!("failed to stat {}", path.display()))?;
  cache::touch_cache_entry_sync(path);
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
    let _ = fs::remove_file(&self.path);
  }
}

fn acquire_page_image_lock(output_path: &Path) -> Result<PageImageLock> {
  let lock_path = lock_path_for(output_path);
  loop {
    match OpenOptions::new()
      .write(true)
      .create_new(true)
      .open(&lock_path)
    {
      Ok(mut file) => {
        let _ = writeln!(file, "pid={}", std::process::id());
        return Ok(PageImageLock { path: lock_path });
      }
      Err(error) if error.kind() == ErrorKind::AlreadyExists => {
        if lock_is_stale(&lock_path) {
          warn!(
            lock = %lock_path.display(),
            "removing stale pdf page image cache lock"
          );
          let _ = fs::remove_file(&lock_path);
          continue;
        }
        thread::sleep(Duration::from_millis(40));
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

fn lock_is_stale(path: &Path) -> bool {
  fs::metadata(path)
    .ok()
    .and_then(|metadata| metadata.modified().ok())
    .and_then(|modified| modified.elapsed().ok())
    .is_some_and(|age| age > Duration::from_secs(600))
}

fn temp_page_prefix(prefix: &Path) -> PathBuf {
  let nanos = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_nanos();
  let mut name = prefix
    .file_name()
    .map(|name| name.to_os_string())
    .unwrap_or_else(|| "page".into());
  name.push(format!(".tmp-{}-{nanos}", std::process::id()));
  prefix.with_file_name(name)
}

fn png_path_for_prefix(prefix: &Path) -> PathBuf {
  let mut name = prefix
    .file_name()
    .map(|name| name.to_os_string())
    .unwrap_or_else(|| "page".into());
  name.push(".png");
  prefix.with_file_name(name)
}

pub(super) fn render_page_slice_image(
  document: &PdfDocument,
  spec: PageSliceSpec,
) -> Result<PageImage> {
  let spec = spec.normalized();
  fs::create_dir_all(&document.page_cache_dir).with_context(|| {
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
  )?;
  let cache_key = document.slice_cache_key(spec);
  let page_number = spec.page_index + 1;
  let output_path = document.page_cache_dir.join(format!(
    "{}-p{page_number:05}-slice{:03}of{:03}-{}x{}-y{}h{}.png",
    cache_key,
    spec.slice_index + 1,
    spec.slice_count,
    spec.target_width,
    spec.target_height,
    spec.slice_y,
    spec.slice_height
  ));
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
    debug!(
      page = page_number,
      slice = spec.slice_index + 1,
      slice_count = spec.slice_count,
      source = %full_page.path.display(),
      path = %output_path.display(),
      slice_y,
      slice_height,
      "rendering pdf page slice image"
    );
    let full_image = image::open(&full_page.path)
      .with_context(|| format!("failed to open {}", full_page.path.display()))?;
    let slice = full_image.crop_imm(0, slice_y, full_width, slice_height);
    slice
      .save(&output_path)
      .with_context(|| format!("failed to write {}", output_path.display()))?;
  } else {
    debug!(
      page = page_number,
      slice = spec.slice_index + 1,
      slice_count = spec.slice_count,
      path = %output_path.display(),
      "using cached pdf page slice image"
    );
  }

  write_slice_metadata(&metadata_path, &metadata)?;
  cache::touch_cache_entry_sync(&output_path);
  cache::touch_cache_entry_sync(&metadata_path);

  let (width, height) = image::image_dimensions(&output_path)
    .with_context(|| format!("failed to read {}", output_path.display()))?;
  let file_metadata = fs::metadata(&output_path)
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

fn write_slice_metadata(path: &Path, metadata: &PageSliceMetadata) -> Result<()> {
  let encoded = toml::to_string_pretty(metadata).context("failed to encode slice metadata")?;
  fs::write(path, encoded).with_context(|| format!("failed to write {}", path.display()))
}
