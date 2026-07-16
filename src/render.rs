mod cache_file;
mod chafa;
mod driver;
mod key;

use std::{
  collections::{HashMap, HashSet},
  path::PathBuf,
  sync::Arc,
};

use img_tui::{NativeImageConfig, RenderMode, native_image};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, mpsc};
use tracing::debug;

use crate::{
  config::RenderConfig,
  event::{AsyncEvent, RenderOutcome, RenderedImage},
  pdf::PageImage,
};

use driver::render_with_fallbacks;
use key::{hash_native_config, hash_render_config, hash_render_kind};

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

#[derive(Debug, Clone)]
pub(super) struct RenderedBytes {
  pub(super) data: Vec<u8>,
  pub(super) refresh: Option<Vec<u8>>,
}
