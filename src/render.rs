use std::{
  collections::{HashMap, HashSet},
  io::{Cursor, Write},
  path::{Path, PathBuf},
  process::Command,
  sync::Arc,
};

use ansi_to_tui::IntoText;
use img_tui::{NativeImageConfig, ProtocolPlacement, RenderMode, native_image};
use ratatui::text::Text;
use sha2::{Digest, Sha256};
use tokio::{
  fs,
  sync::{Mutex, OwnedSemaphorePermit, Semaphore, mpsc},
};
use tracing::debug;

use crate::{
  cache,
  config::RenderConfig,
  event::{AsyncEvent, RenderOutcome, RenderedImage},
  pdf::PageImage,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RenderKind {
  Fit,
}

type PreparedImageCache = Arc<Mutex<HashMap<String, native_image::PreparedNativeImage>>>;

pub struct RenderStore {
  cache_dir: PathBuf,
  config: RenderConfig,
  native_config: NativeImageConfig,
  modes: Vec<RenderMode>,
  memory: HashMap<String, RenderedImage>,
  last_success: HashMap<String, String>,
  failures: HashMap<String, String>,
  in_flight: HashSet<String>,
  in_flight_slots: HashMap<String, String>,
  visible_render_waits: HashSet<String>,
  prepared_images: PreparedImageCache,
  max_concurrent: usize,
  semaphore: Arc<Semaphore>,
  preload_semaphore: Arc<Semaphore>,
}

struct RenderPermits {
  _global: OwnedSemaphorePermit,
  _preload: Option<OwnedSemaphorePermit>,
}

impl RenderStore {
  pub fn new(
    cache_dir: PathBuf,
    config: RenderConfig,
    native_config: NativeImageConfig,
    modes: Vec<RenderMode>,
  ) -> Self {
    let max_concurrent = config.max_concurrent.max(1);
    let max_preloads = max_concurrent.saturating_sub(1);
    Self {
      cache_dir,
      config,
      native_config,
      modes,
      memory: HashMap::new(),
      last_success: HashMap::new(),
      failures: HashMap::new(),
      in_flight: HashSet::new(),
      in_flight_slots: HashMap::new(),
      visible_render_waits: HashSet::new(),
      prepared_images: Arc::new(Mutex::new(HashMap::new())),
      max_concurrent,
      semaphore: Arc::new(Semaphore::new(max_concurrent)),
      preload_semaphore: Arc::new(Semaphore::new(max_preloads)),
    }
  }

  pub fn request(
    &mut self,
    page: &PageImage,
    width: u16,
    height: u16,
    kind: RenderKind,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) -> RenderRequest {
    self.request_with_permits(page, width, height, kind, tx, None, false)
  }

  pub fn preload(
    &mut self,
    page: &PageImage,
    width: u16,
    height: u16,
    kind: RenderKind,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) {
    if self.in_flight.len() >= self.max_concurrent {
      debug!(
        page = page.page_index + 1,
        width,
        height,
        ?kind,
        in_flight = self.in_flight.len(),
        max_concurrent = self.max_concurrent,
        "render preload skipped because renderer is saturated"
      );
      return;
    }
    let Some(permits) = self.try_preload_permits() else {
      debug!(
        page = page.page_index + 1,
        width,
        height,
        ?kind,
        "render preload skipped because permits are unavailable"
      );
      return;
    };
    self.request_with_permits(page, width, height, kind, tx, Some(permits), true);
  }

  fn request_with_permits(
    &mut self,
    page: &PageImage,
    width: u16,
    height: u16,
    kind: RenderKind,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
    permits: Option<RenderPermits>,
    preload: bool,
  ) -> RenderRequest {
    let cache_key = self.cache_key(page, width, height, kind);
    let slot_key = self.slot_key(page, width, height, kind);
    if width == 0 || height == 0 {
      debug!(
        page = page.page_index + 1,
        width,
        height,
        ?kind,
        "render request ignored because area is empty"
      );
      return RenderRequest {
        cache_key,
        slot_key,
      };
    }
    if self.memory.contains_key(&cache_key) || self.in_flight.contains(&cache_key) {
      if self.in_flight.contains(&cache_key) && !preload {
        self.visible_render_waits.insert(cache_key.clone());
      }
      debug!(
        page = page.page_index + 1,
        width,
        height,
        ?kind,
        cache_key = %cache_key,
        slot_key = %slot_key,
        preload,
        memory = self.memory.contains_key(&cache_key),
        in_flight = self.in_flight.contains(&cache_key),
        "render request reused existing state"
      );
      return RenderRequest {
        cache_key,
        slot_key,
      };
    }
    if self.failures.contains_key(&cache_key) && !preload {
      debug!(
        page = page.page_index + 1,
        width,
        height,
        ?kind,
        cache_key = %cache_key,
        "render request skipped because failure is cached"
      );
      return RenderRequest {
        cache_key,
        slot_key,
      };
    }
    if let Some(in_flight_key) = self.in_flight_slots.get(&slot_key)
      && (preload || in_flight_key == &cache_key)
    {
      if !preload {
        self.visible_render_waits.insert(in_flight_key.clone());
      }
      debug!(
        page = page.page_index + 1,
        width,
        height,
        ?kind,
        cache_key = %cache_key,
        slot_key = %slot_key,
        in_flight_key = %in_flight_key,
        preload,
        "render request skipped because slot has in-flight render"
      );
      return RenderRequest {
        cache_key,
        slot_key,
      };
    }

    self.in_flight.insert(cache_key.clone());
    self
      .in_flight_slots
      .insert(slot_key.clone(), cache_key.clone());
    let cache_dir = self.cache_dir.clone();
    let page = page.clone();
    let config = self.config.clone();
    let native_config = self.native_config.clone();
    let modes = self.modes.clone();
    let prepared_images = self.prepared_images.clone();
    let semaphore = self.semaphore.clone();
    let tx = tx.clone();
    let outcome_key = cache_key.clone();
    let outcome_slot_key = slot_key.clone();
    debug!(
      page = page.page_index + 1,
      width,
      height,
      ?kind,
      cache_key = %cache_key,
      slot_key = %slot_key,
      preload,
      "spawned render request"
    );
    tokio::spawn(async move {
      let result = render_with_fallbacks(
        page,
        width,
        height,
        kind,
        cache_dir,
        config,
        native_config,
        modes,
        prepared_images,
        semaphore,
        permits,
      )
      .await;
      let _ = tx.send(AsyncEvent::Render(RenderOutcome {
        cache_key: outcome_key,
        slot_key: outcome_slot_key,
        preload,
        result,
      }));
    });
    RenderRequest {
      cache_key,
      slot_key,
    }
  }

  fn try_preload_permits(&self) -> Option<RenderPermits> {
    let preload = self.preload_semaphore.clone().try_acquire_owned().ok()?;
    let global = self.semaphore.clone().try_acquire_owned().ok()?;
    Some(RenderPermits {
      _global: global,
      _preload: Some(preload),
    })
  }

  pub fn get(&self, cache_key: &str) -> Option<&RenderedImage> {
    self.memory.get(cache_key)
  }

  pub fn failure(&self, cache_key: &str) -> Option<&str> {
    self.failures.get(cache_key).map(String::as_str)
  }

  pub fn draws_with_protocol(&self) -> bool {
    self.modes.first().is_some_and(|mode| mode.is_protocol())
  }

  pub fn clear_state(&mut self) {
    self.memory.clear();
    self.last_success.clear();
    self.failures.clear();
    self.in_flight.clear();
    self.in_flight_slots.clear();
    self.visible_render_waits.clear();
    self.prepared_images = Arc::new(Mutex::new(HashMap::new()));
  }

  pub fn rendered_key(
    &self,
    cache_key: &str,
    slot_key: &str,
    allow_fallback: bool,
  ) -> Option<String> {
    if self.memory.contains_key(cache_key) {
      return Some(cache_key.to_string());
    }
    if !allow_fallback {
      return None;
    }
    self
      .last_success
      .get(slot_key)
      .filter(|fallback_key| self.memory.contains_key(*fallback_key))
      .cloned()
  }

  pub fn mark_drawn(&mut self, _cache_key: &str) {}

  pub fn take_protocol_writes(
    &mut self,
    _drawn_render_keys: &[String],
    _include_background: bool,
  ) -> Vec<String> {
    Vec::new()
  }

  pub fn finish(&mut self, outcome: RenderOutcome) -> RenderFinish {
    self.in_flight.remove(&outcome.cache_key);
    let visible_wait = self.visible_render_waits.remove(&outcome.cache_key);
    if self
      .in_flight_slots
      .get(&outcome.slot_key)
      .is_some_and(|cache_key| cache_key == &outcome.cache_key)
    {
      self.in_flight_slots.remove(&outcome.slot_key);
    }
    match outcome.result {
      Ok(rendered) => {
        debug!(
          cache_key = %outcome.cache_key,
          slot_key = %outcome.slot_key,
          preload = outcome.preload,
          visible_wait,
          "render finish success"
        );
        self.failures.remove(&outcome.cache_key);
        self
          .last_success
          .insert(outcome.slot_key, outcome.cache_key.clone());
        self.memory.insert(outcome.cache_key, rendered);
        RenderFinish {
          message: None,
          needs_draw: !outcome.preload || visible_wait,
        }
      }
      Err(error) => {
        debug!(
          cache_key = %outcome.cache_key,
          slot_key = %outcome.slot_key,
          preload = outcome.preload,
          visible_wait,
          %error,
          "render finish error"
        );
        if outcome.preload {
          return RenderFinish {
            message: None,
            needs_draw: visible_wait,
          };
        }
        self
          .failures
          .insert(outcome.cache_key.clone(), error.clone());
        RenderFinish {
          message: Some(format!("render failed: {error}")),
          needs_draw: true,
        }
      }
    }
  }

  fn cache_key(&self, page: &PageImage, width: u16, height: u16, kind: RenderKind) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"pdf-tui-render-v1");
    hasher.update(page.path.to_string_lossy().as_bytes());
    hasher.update(page.page_index.to_le_bytes());
    hasher.update(page.size_bytes.to_le_bytes());
    hasher.update(page.modified_nanos.to_le_bytes());
    hasher.update(width.to_le_bytes());
    hasher.update(height.to_le_bytes());
    hash_render_kind(&mut hasher, kind, true);
    hash_render_config(&mut hasher, &self.config);
    hash_native_config(&mut hasher, &self.native_config);
    for mode in &self.modes {
      hasher.update(mode.label().as_bytes());
      hasher.update([0]);
    }
    hex::encode(hasher.finalize())
  }

  fn slot_key(&self, page: &PageImage, width: u16, height: u16, kind: RenderKind) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"pdf-tui-slot-v1");
    hasher.update(page.path.to_string_lossy().as_bytes());
    hasher.update(page.page_index.to_le_bytes());
    hasher.update(page.size_bytes.to_le_bytes());
    hasher.update(page.modified_nanos.to_le_bytes());
    hasher.update(width.to_le_bytes());
    hasher.update(height.to_le_bytes());
    hash_render_kind(&mut hasher, kind, false);
    hash_render_config(&mut hasher, &self.config);
    hash_native_config(&mut hasher, &self.native_config);
    for mode in &self.modes {
      hasher.update(mode.label().as_bytes());
      hasher.update([0]);
    }
    hex::encode(hasher.finalize())
  }
}

pub struct RenderRequest {
  pub cache_key: String,
  pub slot_key: String,
}

pub struct RenderFinish {
  pub message: Option<String>,
  pub needs_draw: bool,
}

#[allow(clippy::too_many_arguments)]
async fn render_with_fallbacks(
  page: PageImage,
  width: u16,
  height: u16,
  kind: RenderKind,
  cache_dir: PathBuf,
  config: RenderConfig,
  native_config: NativeImageConfig,
  modes: Vec<RenderMode>,
  prepared_images: PreparedImageCache,
  semaphore: Arc<Semaphore>,
  permits: Option<RenderPermits>,
) -> Result<RenderedImage, String> {
  let _permits = acquire_render_permits(semaphore, permits).await?;

  let mut errors = Vec::new();
  for mode in modes {
    let image_id = kitty_image_id(&page, width, height, mode);
    let placement_id = kitty_placement_id(&page, mode, image_id);
    let rendered = render_or_read_cache(
      &page,
      &cache_dir,
      width,
      height,
      kind,
      &config,
      &native_config,
      mode,
      image_id,
      placement_id,
      &prepared_images,
    )
    .await;
    match rendered {
      Ok(rendered) => return Ok(rendered),
      Err(error) => errors.push(format!("{}: {error}", mode.label())),
    }
  }
  Err(errors.join("; "))
}

async fn acquire_render_permits(
  semaphore: Arc<Semaphore>,
  permits: Option<RenderPermits>,
) -> Result<RenderPermits, String> {
  match permits {
    Some(permits) => Ok(permits),
    None => Ok(RenderPermits {
      _global: semaphore
        .acquire_owned()
        .await
        .map_err(|error| error.to_string())?,
      _preload: None,
    }),
  }
}

#[allow(clippy::too_many_arguments)]
async fn render_or_read_cache(
  page: &PageImage,
  cache_dir: &Path,
  width: u16,
  height: u16,
  kind: RenderKind,
  config: &RenderConfig,
  native_config: &NativeImageConfig,
  mode: RenderMode,
  image_id: Option<u32>,
  placement_id: Option<u32>,
  prepared_images: &PreparedImageCache,
) -> Result<RenderedImage, String> {
  let cache_path = cache_dir.join(format!(
    "{}.ansi",
    render_cache_key(page, width, height, kind, config, native_config, mode)
  ));

  if let Ok(bytes) = fs::read(&cache_path).await {
    match decode_cache_file(
      &bytes,
      width,
      height,
      native_config.cell_pixels,
      mode,
      image_id,
      placement_id,
    )
    .await
    {
      Ok(decoded) => {
        if decoded.should_rewrite {
          rewrite_cache_file(
            &cache_path,
            &decoded.payload,
            width,
            height,
            native_config.cell_pixels,
            mode,
            image_id,
            placement_id,
            config,
          )
          .await;
        }
        cache::touch_cache_entry(&cache_path).await;
        return decode_rendered(
          decoded.payload,
          mode,
          native_config,
          decoded.image_id,
          decoded.placement_id,
        );
      }
      Err(_) => {}
    }
  }

  let bytes = render_bytes(
    page,
    width,
    height,
    kind,
    config,
    native_config,
    mode,
    image_id,
    placement_id,
    prepared_images,
  )
  .await?;

  let _ = write_cache_file(
    &cache_path,
    &bytes,
    width,
    height,
    native_config.cell_pixels,
    mode,
    image_id,
    placement_id,
    config,
  )
  .await;

  decode_rendered(bytes, mode, native_config, image_id, placement_id)
}

#[allow(clippy::too_many_arguments)]
async fn render_bytes(
  page: &PageImage,
  width: u16,
  height: u16,
  kind: RenderKind,
  config: &RenderConfig,
  native_config: &NativeImageConfig,
  mode: RenderMode,
  image_id: Option<u32>,
  placement_id: Option<u32>,
  prepared_images: &PreparedImageCache,
) -> Result<RenderedBytes, String> {
  let source_path = page.path.clone();
  if mode.is_protocol() {
    let prepared =
      prepared_native_image(page, width, height, kind, native_config, prepared_images).await?;
    if mode == RenderMode::Kitty
      && let Some(placement_id) = placement_id
    {
      let viewport = native_image::NativeImageViewport {
        full_width_cells: width,
        full_height_cells: height,
        visible_width_cells: width,
        visible_height_cells: height,
        left_cells: 0,
        top_cells: 0,
      };
      let image_id = image_id.unwrap_or(1);
      let upload = native_image::render_prepared_kitty_upload(&prepared, native_config, image_id)
        .await
        .map_err(|error| error.to_string())?;
      let refresh = native_image::render_kitty_viewport_from_upload(
        &upload,
        viewport,
        native_config,
        placement_id,
      )
      .map_err(|error| error.to_string())?;
      Ok(RenderedBytes {
        data: upload.data,
        refresh: Some(refresh),
      })
    } else {
      native_image::render_prepared(&prepared, mode, native_config, image_id)
        .await
        .map(|data| RenderedBytes {
          data,
          refresh: None,
        })
        .map_err(|error| error.to_string())
    }
  } else {
    run_chafa(&source_path, width, height, config, mode)
      .await
      .map(|data| RenderedBytes {
        data,
        refresh: None,
      })
  }
}

fn prepared_dimensions(_kind: RenderKind, width: u16, height: u16) -> (u16, u16) {
  (width, height)
}

fn prepared_cache_key(
  page: &PageImage,
  width: u16,
  height: u16,
  kind: RenderKind,
  native_config: &NativeImageConfig,
) -> String {
  let (prepared_width, prepared_height) = prepared_dimensions(kind, width, height);
  let mut hasher = Sha256::new();
  hasher.update(b"pdf-tui-prepared-image-v2");
  hasher.update(page.path.to_string_lossy().as_bytes());
  hasher.update(page.page_index.to_le_bytes());
  hasher.update(page.size_bytes.to_le_bytes());
  hasher.update(page.modified_nanos.to_le_bytes());
  hasher.update(prepared_width.to_le_bytes());
  hasher.update(prepared_height.to_le_bytes());
  hasher.update(native_config.cell_pixels.unwrap_or((0, 0)).0.to_le_bytes());
  hasher.update(native_config.cell_pixels.unwrap_or((0, 0)).1.to_le_bytes());
  hex::encode(hasher.finalize())
}

async fn prepared_native_image(
  page: &PageImage,
  width: u16,
  height: u16,
  kind: RenderKind,
  native_config: &NativeImageConfig,
  cache: &PreparedImageCache,
) -> Result<native_image::PreparedNativeImage, String> {
  let key = prepared_cache_key(page, width, height, kind, native_config);
  let mut cache = cache.lock().await;
  if let Some(prepared) = cache.get(&key) {
    return Ok(prepared.clone());
  }

  let (prepared_width, prepared_height) = prepared_dimensions(kind, width, height);
  let prepared = native_image::prepare(
    &page.path,
    prepared_width,
    prepared_height,
    native_config.cell_pixels,
  )
  .await
  .map_err(|error| error.to_string())?;
  cache.insert(key, prepared.clone());
  Ok(prepared)
}

async fn run_chafa(
  image_path: &Path,
  width: u16,
  height: u16,
  config: &RenderConfig,
  mode: RenderMode,
) -> Result<Vec<u8>, String> {
  let mut command = chafa_command(width, height, config, mode)?;
  command.arg(image_path);

  let chafa_bin = config.chafa_bin.clone();
  let output = tokio::task::spawn_blocking(move || command.output())
    .await
    .map_err(|error| format!("chafa worker failed: {error}"))?
    .map_err(|error| format!("failed to run {chafa_bin}: {error}"))?;
  check_chafa_output(output, &config.chafa_bin)
}

fn chafa_command(
  width: u16,
  height: u16,
  config: &RenderConfig,
  mode: RenderMode,
) -> Result<Command, String> {
  if mode.is_protocol() {
    return Err(format!(
      "{} must be rendered by native image driver, not chafa",
      mode.label()
    ));
  }

  let mut command = Command::new(&config.chafa_bin);
  let mut args: Vec<String> = config
    .chafa_args
    .iter()
    .filter(|arg| {
      !arg.starts_with("--format=")
        && !arg.starts_with("--colors=")
        && !arg.starts_with("--symbols=")
        && !arg.starts_with("--passthrough=")
        && !arg.starts_with("--probe=")
        && !arg.starts_with("--relative=")
    })
    .cloned()
    .collect();

  args.push(format!("--format={}", mode.chafa_format()));
  args.push("--probe=off".to_string());
  args.push("--relative=off".to_string());
  args.push("--passthrough=none".to_string());
  if !args.iter().any(|arg| arg.starts_with("--scale=")) {
    args.push("--scale=max".to_string());
  }
  if config.chafa_threads > 0
    && !config
      .chafa_args
      .iter()
      .any(|arg| arg.starts_with("--threads="))
  {
    args.push(format!("--threads={}", config.chafa_threads));
  }
  match mode {
    RenderMode::Symbols => {
      for arg in &config.chafa_args {
        if arg.starts_with("--colors=") || arg.starts_with("--symbols=") {
          args.push(arg.clone());
        }
      }
    }
    RenderMode::Ascii => {
      args.push("--colors=none".to_string());
      args.push("--symbols=ascii".to_string());
    }
    _ => {}
  }

  command
    .args(args)
    .arg("--size")
    .arg(format!("{width}x{height}"));

  Ok(command)
}

fn check_chafa_output(output: std::process::Output, chafa_bin: &str) -> Result<Vec<u8>, String> {
  if !output.status.success() {
    let stderr = String::from_utf8_lossy(&output.stderr);
    return Err(format!(
      "{chafa_bin} exited with {}: {}",
      output.status,
      stderr.trim()
    ));
  }
  Ok(output.stdout)
}

const CACHE_MAGIC: &str = "pdf-tui-render-cache-v4";
const LEGACY_ZSTD_CACHE_MAGIC: &str = "pdf-tui-render-cache-v2";
const LEGACY_RAW_CACHE_MAGIC: &str = "pdf-tui-render-cache-v1";
const FRAMED_PAYLOAD_MAGIC: &[u8] = b"pdf-tui-rendered-bytes-v1\0";

#[derive(Debug, Clone)]
struct RenderedBytes {
  data: Vec<u8>,
  refresh: Option<Vec<u8>>,
}

struct DecodedCacheFile {
  payload: RenderedBytes,
  image_id: Option<u32>,
  placement_id: Option<u32>,
  should_rewrite: bool,
}

#[allow(clippy::too_many_arguments)]
async fn write_cache_file(
  cache_path: &Path,
  payload: &RenderedBytes,
  width: u16,
  height: u16,
  cell_pixels: Option<(u16, u16)>,
  mode: RenderMode,
  image_id: Option<u32>,
  placement_id: Option<u32>,
  config: &RenderConfig,
) -> Result<(), String> {
  if let Some(parent) = cache_path.parent() {
    fs::create_dir_all(parent)
      .await
      .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
  }
  let cached = encode_cache_file(
    payload,
    width,
    height,
    cell_pixels,
    mode,
    image_id,
    placement_id,
    config,
  )
  .await
  .map_err(|error| format!("failed to encode cache {}: {error}", cache_path.display()))?;
  fs::write(cache_path, cached)
    .await
    .map_err(|error| format!("failed to write cache {}: {error}", cache_path.display()))?;
  cache::touch_cache_entry(cache_path).await;
  Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn rewrite_cache_file(
  cache_path: &Path,
  payload: &RenderedBytes,
  width: u16,
  height: u16,
  cell_pixels: Option<(u16, u16)>,
  mode: RenderMode,
  image_id: Option<u32>,
  placement_id: Option<u32>,
  config: &RenderConfig,
) {
  let _ = write_cache_file(
    cache_path,
    payload,
    width,
    height,
    cell_pixels,
    mode,
    image_id,
    placement_id,
    config,
  )
  .await;
}

async fn encode_cache_file(
  payload: &RenderedBytes,
  width: u16,
  height: u16,
  cell_pixels: Option<(u16, u16)>,
  mode: RenderMode,
  image_id: Option<u32>,
  placement_id: Option<u32>,
  config: &RenderConfig,
) -> Result<Vec<u8>, String> {
  let compression_level = config.cache_compression_level;
  let compression_threads = config.cache_compression_threads;
  let payload_format = if payload.refresh.is_some() {
    "framed"
  } else {
    "raw"
  };
  let payload = encode_rendered_bytes(payload)?;
  let plain_len = payload.len();
  let compressed = tokio::task::spawn_blocking(move || {
    compress_zstd(payload, compression_level, compression_threads)
  })
  .await
  .map_err(|error| format!("zstd compression worker failed: {error}"))?
  .map_err(|error| format!("zstd compression failed: {error}"))?;

  let (cell_width, cell_height) = cell_pixels.unwrap_or((0, 0));
  let mut header = format!(
    "{CACHE_MAGIC}\nwidth={width}\nheight={height}\ncell_width={cell_width}\ncell_height={cell_height}\nmode={}\ncompression=zstd\npayload_format={payload_format}\nuncompressed_bytes={plain_len}\n",
    mode.label()
  );
  if let Some(image_id) = image_id {
    header.push_str(&format!("image_id={image_id}\n"));
  }
  if let Some(placement_id) = placement_id {
    header.push_str(&format!("placement_id={placement_id}\n"));
  }
  header.push('\n');
  let mut out = Vec::with_capacity(header.len() + compressed.len());
  out.extend_from_slice(header.as_bytes());
  out.extend_from_slice(&compressed);
  Ok(out)
}

async fn decode_cache_file(
  bytes: &[u8],
  expected_width: u16,
  expected_height: u16,
  expected_cell_pixels: Option<(u16, u16)>,
  expected_mode: RenderMode,
  expected_image_id: Option<u32>,
  expected_placement_id: Option<u32>,
) -> Result<DecodedCacheFile, String> {
  let header_end = bytes
    .windows(2)
    .position(|window| window == b"\n\n")
    .ok_or_else(|| "cache metadata header missing".to_string())?;
  let header = std::str::from_utf8(&bytes[..header_end])
    .map_err(|error| format!("cache metadata is not utf-8: {error}"))?;
  let mut lines = header.lines();
  let magic = lines
    .next()
    .ok_or_else(|| "cache metadata magic missing".to_string())?;
  if magic != CACHE_MAGIC && magic != LEGACY_ZSTD_CACHE_MAGIC && magic != LEGACY_RAW_CACHE_MAGIC {
    return Err("cache metadata magic mismatch".to_string());
  }

  let mut width = None;
  let mut height = None;
  let mut cell_width = None;
  let mut cell_height = None;
  let mut mode = None;
  let mut compression = None;
  let mut payload_format = None;
  let mut uncompressed_bytes = None;
  let mut image_id = None;
  let mut placement_id = None;
  for line in lines {
    if let Some(value) = line.strip_prefix("width=") {
      width = value.parse::<u16>().ok();
    } else if let Some(value) = line.strip_prefix("height=") {
      height = value.parse::<u16>().ok();
    } else if let Some(value) = line.strip_prefix("cell_width=") {
      cell_width = value.parse::<u16>().ok();
    } else if let Some(value) = line.strip_prefix("cell_height=") {
      cell_height = value.parse::<u16>().ok();
    } else if let Some(value) = line.strip_prefix("mode=") {
      mode = Some(value);
    } else if let Some(value) = line.strip_prefix("compression=") {
      compression = Some(value);
    } else if let Some(value) = line.strip_prefix("payload_format=") {
      payload_format = Some(value);
    } else if let Some(value) = line.strip_prefix("uncompressed_bytes=") {
      uncompressed_bytes = value.parse::<usize>().ok();
    } else if let Some(value) = line.strip_prefix("image_id=") {
      image_id = value.parse::<u32>().ok();
    } else if let Some(value) = line.strip_prefix("placement_id=") {
      placement_id = value.parse::<u32>().ok();
    }
  }

  if width != Some(expected_width) || height != Some(expected_height) {
    return Err(format!(
      "cache size mismatch: got {:?}x{:?}, expected {}x{}",
      width, height, expected_width, expected_height
    ));
  }
  if mode != Some(expected_mode.label()) {
    return Err(format!(
      "cache mode mismatch: got {:?}, expected {}",
      mode,
      expected_mode.label()
    ));
  }
  let (expected_cell_width, expected_cell_height) = expected_cell_pixels.unwrap_or((0, 0));
  if cell_width != Some(expected_cell_width) || cell_height != Some(expected_cell_height) {
    return Err(format!(
      "cache cell size mismatch: got {:?}x{:?}, expected {}x{}",
      cell_width, cell_height, expected_cell_width, expected_cell_height
    ));
  }
  if image_id != expected_image_id {
    return Err(format!(
      "cache image id mismatch: got {:?}, expected {:?}",
      image_id, expected_image_id
    ));
  }
  if placement_id != expected_placement_id {
    return Err(format!(
      "cache placement id mismatch: got {:?}, expected {:?}",
      placement_id, expected_placement_id
    ));
  }

  let payload = &bytes[header_end + 2..];
  let payload_format = payload_format.unwrap_or("raw");
  match compression.unwrap_or("none") {
    "none" => Ok(DecodedCacheFile {
      payload: decode_rendered_bytes(payload.to_vec(), payload_format)?,
      image_id,
      placement_id,
      should_rewrite: magic != CACHE_MAGIC || payload_format != "raw",
    }),
    "zstd" => {
      let expected_len = uncompressed_bytes;
      let payload = payload.to_vec();
      let decoded = tokio::task::spawn_blocking(move || decompress_zstd(payload))
        .await
        .map_err(|error| format!("zstd decompression worker failed: {error}"))?
        .map_err(|error| format!("zstd decompression failed: {error}"))?;
      if let Some(expected_len) = expected_len
        && decoded.len() != expected_len
      {
        return Err(format!(
          "cache decompressed size mismatch: got {}, expected {}",
          decoded.len(),
          expected_len
        ));
      }
      Ok(DecodedCacheFile {
        payload: decode_rendered_bytes(decoded, payload_format)?,
        image_id,
        placement_id,
        should_rewrite: magic != CACHE_MAGIC,
      })
    }
    value => Err(format!("unsupported cache compression: {value}")),
  }
}

fn compress_zstd(payload: Vec<u8>, level: i32, threads: u32) -> std::io::Result<Vec<u8>> {
  let mut encoder = zstd::stream::Encoder::new(Vec::new(), level)?;
  if threads > 0 {
    encoder.multithread(threads)?;
  }
  encoder.write_all(&payload)?;
  encoder.finish()
}

fn decompress_zstd(payload: Vec<u8>) -> std::io::Result<Vec<u8>> {
  zstd::stream::decode_all(Cursor::new(payload))
}

fn encode_rendered_bytes(payload: &RenderedBytes) -> Result<Vec<u8>, String> {
  let Some(refresh) = &payload.refresh else {
    return Ok(payload.data.clone());
  };
  let data_len = u64::try_from(payload.data.len())
    .map_err(|_| "render payload is too large to cache".to_string())?;
  let refresh_len = u64::try_from(refresh.len())
    .map_err(|_| "refresh payload is too large to cache".to_string())?;
  let mut out = Vec::with_capacity(
    FRAMED_PAYLOAD_MAGIC.len() + 16 + payload.data.len().saturating_add(refresh.len()),
  );
  out.extend_from_slice(FRAMED_PAYLOAD_MAGIC);
  out.extend_from_slice(&data_len.to_le_bytes());
  out.extend_from_slice(&refresh_len.to_le_bytes());
  out.extend_from_slice(&payload.data);
  out.extend_from_slice(refresh);
  Ok(out)
}

fn decode_rendered_bytes(bytes: Vec<u8>, payload_format: &str) -> Result<RenderedBytes, String> {
  if payload_format != "framed" {
    return Ok(RenderedBytes {
      data: bytes,
      refresh: None,
    });
  }
  let header_len = FRAMED_PAYLOAD_MAGIC.len() + 16;
  if bytes.len() < header_len || !bytes.starts_with(FRAMED_PAYLOAD_MAGIC) {
    return Err("framed render payload magic mismatch".to_string());
  }
  let lengths = &bytes[FRAMED_PAYLOAD_MAGIC.len()..header_len];
  let data_len = u64::from_le_bytes(
    lengths[0..8]
      .try_into()
      .map_err(|_| "render payload data length missing".to_string())?,
  );
  let refresh_len = u64::from_le_bytes(
    lengths[8..16]
      .try_into()
      .map_err(|_| "render payload refresh length missing".to_string())?,
  );
  let data_len =
    usize::try_from(data_len).map_err(|_| "render payload data length is too large".to_string())?;
  let refresh_len = usize::try_from(refresh_len)
    .map_err(|_| "render payload refresh length is too large".to_string())?;
  let data_start = header_len;
  let refresh_start = data_start.saturating_add(data_len);
  let end = refresh_start.saturating_add(refresh_len);
  if end != bytes.len() {
    return Err(format!(
      "framed render payload size mismatch: got {}, expected {}",
      bytes.len(),
      end
    ));
  }
  Ok(RenderedBytes {
    data: bytes[data_start..refresh_start].to_vec(),
    refresh: Some(bytes[refresh_start..end].to_vec()),
  })
}

fn decode_rendered(
  bytes: RenderedBytes,
  mode: RenderMode,
  native_config: &NativeImageConfig,
  image_id: Option<u32>,
  placement_id: Option<u32>,
) -> Result<RenderedImage, String> {
  decode_rendered_with_refresh(bytes, mode, native_config, image_id, placement_id)
}

fn decode_rendered_with_refresh(
  bytes: RenderedBytes,
  mode: RenderMode,
  native_config: &NativeImageConfig,
  image_id: Option<u32>,
  placement_id: Option<u32>,
) -> Result<RenderedImage, String> {
  if mode.is_protocol() {
    let fingerprint = render_fingerprint(&bytes.data);
    let data = String::from_utf8(bytes.data).map_err(|error| error.to_string())?;
    let refresh = bytes
      .refresh
      .map(String::from_utf8)
      .transpose()
      .map_err(|error| error.to_string())?;
    let placement = match (
      mode,
      native_config.kitty_unicode_placeholders,
      image_id,
      placement_id,
    ) {
      (RenderMode::Kitty, _, Some(image_id), Some(placement_id)) => {
        Some(ProtocolPlacement::KittyPlacement {
          image_id,
          placement_id,
        })
      }
      (RenderMode::Kitty, true, Some(image_id), None) => {
        Some(ProtocolPlacement::KittyUnicode { image_id })
      }
      _ => None,
    };
    let erase = if mode == RenderMode::Kitty
      && let (Some(image_id), Some(placement_id)) = (image_id, placement_id)
    {
      native_image::erase_kitty_placement_sequence(
        native_config.passthrough.as_deref(),
        image_id,
        placement_id,
      )
    } else {
      native_image::erase_sequence(mode, native_config.passthrough.as_deref(), image_id)
    };
    Ok(RenderedImage::Protocol {
      mode,
      data,
      refresh,
      placement,
      fingerprint,
      erase,
    })
  } else {
    let text: Text<'static> = bytes.data.into_text().map_err(|error| error.to_string())?;
    Ok(RenderedImage::Symbols { mode, text })
  }
}

fn hash_render_kind(hasher: &mut Sha256, kind: RenderKind, _include_viewport_offset: bool) {
  match kind {
    RenderKind::Fit => hasher.update(b"fit"),
  }
}

fn render_cache_key(
  page: &PageImage,
  width: u16,
  height: u16,
  kind: RenderKind,
  config: &RenderConfig,
  native_config: &NativeImageConfig,
  mode: RenderMode,
) -> String {
  let mut hasher = Sha256::new();
  hasher.update(b"pdf-tui-render-cache-key-v4");
  hasher.update(page.path.to_string_lossy().as_bytes());
  hasher.update(page.page_index.to_le_bytes());
  hasher.update(page.size_bytes.to_le_bytes());
  hasher.update(page.modified_nanos.to_le_bytes());
  hasher.update(width.to_le_bytes());
  hasher.update(height.to_le_bytes());
  hash_render_kind(&mut hasher, kind, true);
  hasher.update(mode.label().as_bytes());
  hasher.update([0]);
  hash_render_config(&mut hasher, config);
  hash_native_config(&mut hasher, native_config);
  for arg in &config.chafa_args {
    hasher.update(arg.as_bytes());
    hasher.update([0]);
  }
  hex::encode(hasher.finalize())
}

fn hash_render_config(hasher: &mut Sha256, config: &RenderConfig) {
  hasher.update(b"render-v2");
  hasher.update(config.chafa_bin.as_bytes());
  hasher.update([0]);
  hasher.update(config.chafa_threads.to_le_bytes());
  hasher.update(config.cache_compression_level.to_le_bytes());
  hasher.update(config.cache_compression_threads.to_le_bytes());
  hasher.update([0]);
  if let Some(passthrough) = &config.passthrough {
    hasher.update(passthrough.as_bytes());
  }
  hasher.update([0]);
  for arg in &config.chafa_args {
    hasher.update(arg.as_bytes());
    hasher.update([0]);
  }
}

fn hash_native_config(hasher: &mut Sha256, config: &NativeImageConfig) {
  hasher.update(config.cell_pixels.unwrap_or((0, 0)).0.to_le_bytes());
  hasher.update(config.cell_pixels.unwrap_or((0, 0)).1.to_le_bytes());
  hasher.update([0]);
  if let Some(passthrough) = &config.passthrough {
    hasher.update(passthrough.as_bytes());
  }
  hasher.update([0]);
  hasher.update([u8::from(config.kitty_unicode_placeholders)]);
  hasher.update([0]);
}

fn kitty_image_id(page: &PageImage, width: u16, height: u16, mode: RenderMode) -> Option<u32> {
  if mode != RenderMode::Kitty {
    return None;
  }
  let mut hasher = Sha256::new();
  hasher.update(b"pdf-tui-kitty-image-v3");
  hasher.update(page.path.to_string_lossy().as_bytes());
  hasher.update(page.page_index.to_le_bytes());
  hasher.update(page.size_bytes.to_le_bytes());
  hasher.update(page.modified_nanos.to_le_bytes());
  hasher.update(width.to_le_bytes());
  hasher.update(height.to_le_bytes());
  hasher.update(mode.label().as_bytes());
  let digest = hasher.finalize();
  let image_id = u32::from_le_bytes(digest[..4].try_into().unwrap_or_default()) & 0x00ff_ffff;
  Some(image_id.max(1))
}

fn kitty_placement_id(page: &PageImage, mode: RenderMode, image_id: Option<u32>) -> Option<u32> {
  if mode == RenderMode::Kitty && page.slice.is_some() {
    image_id
  } else {
    None
  }
}

fn render_fingerprint(bytes: &[u8]) -> u64 {
  let mut hasher = Sha256::new();
  hasher.update(bytes);
  let digest = hasher.finalize();
  u64::from_le_bytes(digest[..8].try_into().unwrap_or_default())
}
