use std::{
  collections::{HashMap, HashSet},
  fs::{self, OpenOptions},
  io::{ErrorKind, Write},
  path::{Path, PathBuf},
  process::Command,
  sync::Arc,
  thread,
  time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tracing::{debug, warn};

use crate::{
  cache,
  config::RenderConfig,
  event::{AsyncEvent, PageOutcome},
};

#[derive(Debug, Clone)]
pub struct PdfDocument {
  pub path: PathBuf,
  pub file_name: String,
  pub page_count: usize,
  pub size_bytes: u64,
  pub modified_nanos: u128,
  pub page_cache_dir: PathBuf,
  pub pdftoppm_bin: String,
  pub dpi: u16,
  pub page_size: Option<(u32, u32)>,
}

#[derive(Debug, Clone)]
pub struct PageImage {
  pub page_index: usize,
  pub path: PathBuf,
  pub width: u32,
  pub height: u32,
  pub size_bytes: u64,
  pub modified_nanos: u128,
  pub slice: Option<PageSliceMetadata>,
}

pub struct PageStore {
  document: PdfDocument,
  in_flight: HashSet<PageRequestKey>,
  visible_waits: HashSet<PageRequestKey>,
  completed: HashMap<usize, PageRequestKey>,
  slice_in_flight: HashSet<PageSliceRequestKey>,
  slice_visible_waits: HashSet<PageSliceRequestKey>,
  slice_completed: HashSet<PageSliceRequestKey>,
  max_concurrent: usize,
  semaphore: Arc<Semaphore>,
  preload_semaphore: Arc<Semaphore>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PageRequestKey {
  page_index: usize,
  target_width: u32,
  target_height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PageSliceId {
  pub page_index: usize,
  pub slice_index: u16,
  pub slice_count: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PageSliceSpec {
  pub page_index: usize,
  pub slice_index: u16,
  pub slice_count: u16,
  pub target_width: u32,
  pub target_height: u32,
  pub slice_y: u32,
  pub slice_height: u32,
  pub cell_width: u16,
  pub cell_height: u16,
  pub full_cell_width: u16,
  pub full_cell_height: u16,
  pub viewport_width: u16,
  pub viewport_height: u16,
  pub scroll_divisor: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PageSliceRequestKey {
  spec: PageSliceSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSliceMetadata {
  pub source_pdf: String,
  pub source_size_bytes: u64,
  pub source_modified_nanos: String,
  pub page_index: usize,
  pub page_number: usize,
  pub slice_index: u16,
  pub slice_count: u16,
  pub full_pixel_width: u32,
  pub full_pixel_height: u32,
  pub slice_x: u32,
  pub slice_y: u32,
  pub slice_width: u32,
  pub slice_height: u32,
  pub cell_width: u16,
  pub cell_height: u16,
  pub full_cell_width: u16,
  pub full_cell_height: u16,
  pub viewport_width: u16,
  pub viewport_height: u16,
  pub scroll_divisor: u16,
  pub cache_key: String,
}

impl PageSliceSpec {
  pub fn id(self) -> PageSliceId {
    PageSliceId {
      page_index: self.page_index,
      slice_index: self.slice_index,
      slice_count: self.slice_count,
    }
  }

  fn normalized(self) -> Self {
    let slice_count = self.slice_count.max(1);
    Self {
      page_index: self.page_index,
      slice_index: self.slice_index.min(slice_count.saturating_sub(1)),
      slice_count,
      target_width: self.target_width.max(1),
      target_height: self.target_height.max(1),
      slice_y: self.slice_y,
      slice_height: self.slice_height.max(1),
      cell_width: self.cell_width.max(1),
      cell_height: self.cell_height.max(1),
      full_cell_width: self.full_cell_width.max(1),
      full_cell_height: self.full_cell_height.max(1),
      viewport_width: self.viewport_width.max(1),
      viewport_height: self.viewport_height.max(1),
      scroll_divisor: self.scroll_divisor.max(1),
    }
  }
}

struct PagePermits {
  _global: OwnedSemaphorePermit,
  _preload: Option<OwnedSemaphorePermit>,
}

impl PdfDocument {
  pub fn open(path: PathBuf, cache_dir: PathBuf, render: &RenderConfig) -> Result<Self> {
    let metadata =
      fs::metadata(&path).with_context(|| format!("failed to stat {}", path.display()))?;
    if !metadata.is_file() {
      bail!("{} is not a file", path.display());
    }
    let pdfinfo = read_pdfinfo(&path, &render.pdfinfo_bin)?;
    let page_count = pdfinfo.page_count;
    if page_count == 0 {
      bail!("{} has no pages", path.display());
    }
    let file_name = path
      .file_name()
      .map(|name| name.to_string_lossy().into_owned())
      .unwrap_or_else(|| path.display().to_string());

    Ok(Self {
      path,
      file_name,
      page_count,
      size_bytes: metadata.len(),
      modified_nanos: modified_nanos(&metadata),
      page_cache_dir: cache_dir,
      pdftoppm_bin: render.pdftoppm_bin.clone(),
      dpi: render.page_dpi,
      page_size: pdfinfo.page_size,
    })
  }

  pub fn logical_page_size(&self) -> (u32, u32) {
    self.page_size.unwrap_or((595, 842))
  }

  fn cache_key(&self, target_width: u32, target_height: u32) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"pdf-tui-page-v3");
    hasher.update(self.path.to_string_lossy().as_bytes());
    hasher.update(self.size_bytes.to_le_bytes());
    hasher.update(self.modified_nanos.to_le_bytes());
    hasher.update(self.dpi.to_le_bytes());
    hasher.update(target_width.to_le_bytes());
    hasher.update(target_height.to_le_bytes());
    hex::encode(hasher.finalize())
  }

  fn slice_cache_key(&self, spec: PageSliceSpec) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"pdf-tui-page-slice-v2");
    hasher.update(self.path.to_string_lossy().as_bytes());
    hasher.update(self.size_bytes.to_le_bytes());
    hasher.update(self.modified_nanos.to_le_bytes());
    hasher.update(self.dpi.to_le_bytes());
    hash_slice_spec(&mut hasher, spec);
    hex::encode(hasher.finalize())
  }
}

impl PageStore {
  pub fn new(document: PdfDocument, max_concurrent: usize) -> Self {
    let max_concurrent = max_concurrent.max(1);
    let max_preloads = max_concurrent.saturating_sub(1);
    Self {
      document,
      in_flight: HashSet::new(),
      visible_waits: HashSet::new(),
      completed: HashMap::new(),
      slice_in_flight: HashSet::new(),
      slice_visible_waits: HashSet::new(),
      slice_completed: HashSet::new(),
      max_concurrent,
      semaphore: Arc::new(Semaphore::new(max_concurrent)),
      preload_semaphore: Arc::new(Semaphore::new(max_preloads)),
    }
  }

  pub fn clear_state(&mut self) {
    self.in_flight.clear();
    self.visible_waits.clear();
    self.completed.clear();
    self.slice_in_flight.clear();
    self.slice_visible_waits.clear();
    self.slice_completed.clear();
  }

  pub fn request_slice(&mut self, spec: PageSliceSpec, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    self.request_slice_with_permits(spec, tx, None, false);
  }

  pub fn preload_slice(&mut self, spec: PageSliceSpec, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    if self.in_flight.len() + self.slice_in_flight.len() >= self.max_concurrent {
      debug!(
        page = spec.page_index + 1,
        slice = spec.slice_index + 1,
        slice_count = spec.slice_count,
        in_flight = self.in_flight.len() + self.slice_in_flight.len(),
        max_concurrent = self.max_concurrent,
        "page slice preload skipped because page store is saturated"
      );
      return;
    }
    let Some(permits) = self.try_preload_permits() else {
      debug!(
        page = spec.page_index + 1,
        slice = spec.slice_index + 1,
        slice_count = spec.slice_count,
        "page slice preload skipped because permits are unavailable"
      );
      return;
    };
    self.request_slice_with_permits(spec, tx, Some(permits), true);
  }

  fn request_slice_with_permits(
    &mut self,
    spec: PageSliceSpec,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
    permits: Option<PagePermits>,
    preload: bool,
  ) {
    let spec = spec.normalized();
    let key = PageSliceRequestKey { spec };
    if spec.page_index >= self.document.page_count || self.slice_completed.contains(&key) {
      debug!(
        page = spec.page_index + 1,
        slice = spec.slice_index + 1,
        slice_count = spec.slice_count,
        target_width = spec.target_width,
        target_height = spec.target_height,
        preload,
        completed = self.slice_completed.contains(&key),
        page_count = self.document.page_count,
        "page slice request ignored"
      );
      return;
    }
    if self.slice_in_flight.contains(&key) {
      if !preload {
        self.slice_visible_waits.insert(key);
      }
      debug!(
        page = spec.page_index + 1,
        slice = spec.slice_index + 1,
        slice_count = spec.slice_count,
        target_width = spec.target_width,
        target_height = spec.target_height,
        preload,
        "page slice request reused in-flight render"
      );
      return;
    }
    self.slice_in_flight.insert(key);
    debug!(
      page = spec.page_index + 1,
      slice = spec.slice_index + 1,
      slice_count = spec.slice_count,
      target_width = spec.target_width,
      target_height = spec.target_height,
      slice_y = spec.slice_y,
      slice_height = spec.slice_height,
      preload,
      "spawned page slice render request"
    );
    let document = self.document.clone();
    let tx = tx.clone();
    let semaphore = self.semaphore.clone();
    tokio::spawn(async move {
      let result = match acquire_page_permits(semaphore, permits).await {
        Ok(permits) => {
          let _permits = permits;
          tokio::task::spawn_blocking(move || {
            render_page_slice_image(&document, spec).map_err(|error| error.to_string())
          })
          .await
          .map_err(|error| format!("page slice worker failed: {error}"))
          .and_then(|result| result)
        }
        Err(error) => Err(error),
      };
      let _ = tx.send(AsyncEvent::Page(PageOutcome {
        page_index: spec.page_index,
        target_width: spec.target_width,
        target_height: spec.target_height,
        slice: Some(spec),
        preload,
        result,
      }));
    });
  }

  pub fn request(
    &mut self,
    page_index: usize,
    target_width: u32,
    target_height: u32,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) {
    self.request_with_permits(page_index, target_width, target_height, tx, None, false);
  }

  pub fn preload(
    &mut self,
    page_index: usize,
    target_width: u32,
    target_height: u32,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) {
    if self.in_flight.len() >= self.max_concurrent {
      debug!(
        page = page_index + 1,
        in_flight = self.in_flight.len(),
        max_concurrent = self.max_concurrent,
        "page preload skipped because page store is saturated"
      );
      return;
    }
    let Some(permits) = self.try_preload_permits() else {
      debug!(
        page = page_index + 1,
        "page preload skipped because permits are unavailable"
      );
      return;
    };
    self.request_with_permits(
      page_index,
      target_width,
      target_height,
      tx,
      Some(permits),
      true,
    );
  }

  fn request_with_permits(
    &mut self,
    page_index: usize,
    target_width: u32,
    target_height: u32,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
    permits: Option<PagePermits>,
    preload: bool,
  ) {
    let key = PageRequestKey {
      page_index,
      target_width: target_width.max(1),
      target_height,
    };
    if page_index >= self.document.page_count
      || self
        .completed
        .get(&page_index)
        .is_some_and(|done| *done == key)
    {
      debug!(
        page = page_index + 1,
        target_width = key.target_width,
        target_height = key.target_height,
        preload,
        completed = self.completed.get(&page_index).is_some(),
        page_count = self.document.page_count,
        "page request ignored"
      );
      return;
    }
    if self.in_flight.contains(&key) {
      if !preload {
        self.visible_waits.insert(key);
      }
      debug!(
        page = page_index + 1,
        target_width = key.target_width,
        target_height = key.target_height,
        preload,
        "page request reused in-flight render"
      );
      return;
    }
    self.in_flight.insert(key);
    debug!(
      page = page_index + 1,
      target_width = key.target_width,
      target_height = key.target_height,
      preload,
      "spawned page render request"
    );
    let document = self.document.clone();
    let tx = tx.clone();
    let semaphore = self.semaphore.clone();
    tokio::spawn(async move {
      let result = match acquire_page_permits(semaphore, permits).await {
        Ok(permits) => {
          let _permits = permits;
          tokio::task::spawn_blocking(move || {
            render_page_image(&document, key).map_err(|error| error.to_string())
          })
          .await
          .map_err(|error| format!("page worker failed: {error}"))
          .and_then(|result| result)
        }
        Err(error) => Err(error),
      };
      let _ = tx.send(AsyncEvent::Page(PageOutcome {
        page_index,
        target_width: key.target_width,
        target_height: key.target_height,
        slice: None,
        preload,
        result,
      }));
    });
  }

  fn try_preload_permits(&self) -> Option<PagePermits> {
    let preload = self.preload_semaphore.clone().try_acquire_owned().ok()?;
    let global = self.semaphore.clone().try_acquire_owned().ok()?;
    Some(PagePermits {
      _global: global,
      _preload: Some(preload),
    })
  }

  pub fn finish(&mut self, page: &PageOutcome, completed: bool) -> bool {
    if let Some(spec) = page.slice {
      let slice_id = spec.id();
      let key = PageSliceRequestKey { spec };
      self.slice_in_flight.remove(&key);
      let visible_wait = self.slice_visible_waits.remove(&key);
      if completed {
        self.slice_completed.insert(key);
      }
      debug!(
        page = page.page_index + 1,
        slice = slice_id.slice_index + 1,
        slice_count = slice_id.slice_count,
        target_width = spec.target_width,
        target_height = spec.target_height,
        completed,
        visible_wait,
        in_flight = self.slice_in_flight.len(),
        "page slice store finish"
      );
      return visible_wait;
    }

    let key = PageRequestKey {
      page_index: page.page_index,
      target_width: page.target_width.max(1),
      target_height: page.target_height,
    };
    self.in_flight.remove(&key);
    let visible_wait = self.visible_waits.remove(&key);
    if completed {
      self.completed.insert(page.page_index, key);
    }
    debug!(
      page = page.page_index + 1,
      target_width = key.target_width,
      target_height = key.target_height,
      completed,
      visible_wait,
      in_flight = self.in_flight.len(),
      "page store finish"
    );
    visible_wait
  }
}

async fn acquire_page_permits(
  semaphore: Arc<Semaphore>,
  permits: Option<PagePermits>,
) -> Result<PagePermits, String> {
  match permits {
    Some(permits) => Ok(permits),
    None => Ok(PagePermits {
      _global: semaphore
        .acquire_owned()
        .await
        .map_err(|error| error.to_string())?,
      _preload: None,
    }),
  }
}

struct PdfInfo {
  page_count: usize,
  page_size: Option<(u32, u32)>,
}

fn read_pdfinfo(path: &Path, pdfinfo_bin: &str) -> Result<PdfInfo> {
  let output = Command::new(pdfinfo_bin)
    .arg(path)
    .output()
    .with_context(|| format!("failed to run {pdfinfo_bin}; install poppler-utils"))?;
  if !output.status.success() {
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!("pdfinfo failed: {}", stderr.trim());
  }
  let stdout = String::from_utf8_lossy(&output.stdout);
  let mut page_count = None;
  let mut page_size = None;
  for line in stdout.lines() {
    if let Some(value) = line.strip_prefix("Pages:") {
      page_count = Some(
        value
          .trim()
          .parse::<usize>()
          .context("failed to parse pdfinfo Pages line")?,
      );
    } else if let Some(value) = line.strip_prefix("Page size:") {
      page_size = parse_page_size(value);
    }
  }
  let Some(page_count) = page_count else {
    bail!("pdfinfo did not report a page count");
  };
  Ok(PdfInfo {
    page_count,
    page_size,
  })
}

fn parse_page_size(value: &str) -> Option<(u32, u32)> {
  let (width, rest) = value.trim().split_once('x')?;
  let height = rest.split_whitespace().next()?;
  let width = width.trim().parse::<f64>().ok()?.round().max(1.0) as u32;
  let height = height.trim().parse::<f64>().ok()?.round().max(1.0) as u32;
  Some((width, height))
}

fn render_page_image(document: &PdfDocument, key: PageRequestKey) -> Result<PageImage> {
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

fn render_page_slice_image(document: &PdfDocument, spec: PageSliceSpec) -> Result<PageImage> {
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

fn hash_slice_spec(hasher: &mut Sha256, spec: PageSliceSpec) {
  let spec = spec.normalized();
  hasher.update(spec.page_index.to_le_bytes());
  hasher.update(spec.slice_index.to_le_bytes());
  hasher.update(spec.slice_count.to_le_bytes());
  hasher.update(spec.target_width.to_le_bytes());
  hasher.update(spec.target_height.to_le_bytes());
  hasher.update(spec.slice_y.to_le_bytes());
  hasher.update(spec.slice_height.to_le_bytes());
  hasher.update(spec.cell_width.to_le_bytes());
  hasher.update(spec.cell_height.to_le_bytes());
  hasher.update(spec.full_cell_width.to_le_bytes());
  hasher.update(spec.full_cell_height.to_le_bytes());
  hasher.update(spec.viewport_width.to_le_bytes());
  hasher.update(spec.viewport_height.to_le_bytes());
  hasher.update(spec.scroll_divisor.to_le_bytes());
}

fn modified_nanos(metadata: &fs::Metadata) -> u128 {
  metadata
    .modified()
    .ok()
    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
    .map(|duration| duration.as_nanos())
    .unwrap_or_default()
}
