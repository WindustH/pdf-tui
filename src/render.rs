mod cache_file;
mod chafa;
mod driver;
mod key;
mod memory;

use std::{
  collections::{BinaryHeap, HashMap, HashSet},
  path::PathBuf,
  sync::Arc,
};

use img_tui::{NativeImageConfig, RenderMode};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, mpsc};
use tracing::debug;

use crate::{
  config::RenderConfig,
  event::{AsyncEvent, RenderOutcome, RenderedImage},
  pdf::PageImage,
};

use driver::render_with_fallbacks;
use key::{hash_native_config, hash_render_config, hash_render_kind};
use memory::{PreparedImageMemoryCache, RenderedImageMemoryCache, memory_cache_bytes};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RenderKind {
  Fit,
}

type PreparedImageCache = Arc<Mutex<PreparedImageMemoryCache>>;

pub struct RenderStore {
  cache_dir: PathBuf,
  config: RenderConfig,
  native_config: NativeImageConfig,
  modes: Vec<RenderMode>,
  memory: RenderedImageMemoryCache,
  last_success: HashMap<String, String>,
  failures: HashMap<String, String>,
  in_flight: HashSet<String>,
  in_flight_slots: HashMap<String, String>,
  visible_render_waits: HashSet<String>,
  priorities: HashMap<String, RenderJobPriority>,
  jobs: HashMap<String, RenderJobState>,
  active: HashSet<String>,
  active_preloads: usize,
  pending: BinaryHeap<PendingRenderJob>,
  sequence: u64,
  prepared_images: PreparedImageCache,
  max_concurrent: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RenderJobPriority {
  TerminalPreload,
  Visible,
}

impl RenderJobPriority {
  fn is_preload(self) -> bool {
    !matches!(self, Self::Visible)
  }
}

#[derive(Clone)]
struct RenderJobState {
  page: PageImage,
  width: u16,
  height: u16,
  kind: RenderKind,
  slot_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingRenderJob {
  priority: RenderJobPriority,
  sequence: u64,
  cache_key: String,
}

impl Ord for PendingRenderJob {
  fn cmp(&self, other: &Self) -> std::cmp::Ordering {
    self
      .priority
      .cmp(&other.priority)
      .then_with(|| other.sequence.cmp(&self.sequence))
  }
}

impl PartialOrd for PendingRenderJob {
  fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
    Some(self.cmp(other))
  }
}

impl RenderStore {
  pub fn new(
    cache_dir: PathBuf,
    config: RenderConfig,
    native_config: NativeImageConfig,
    modes: Vec<RenderMode>,
  ) -> Self {
    let max_concurrent = config.max_concurrent.max(1);
    let raw_memory_max_bytes = memory_cache_bytes(config.raw_memory_cache_max_bytes);
    let compressed_memory_max_bytes = memory_cache_bytes(config.compressed_memory_cache_max_bytes);
    let prepared_memory_max_bytes = memory_cache_bytes(config.prepared_memory_cache_max_bytes);
    let memory_compression = config.memory_compression;
    Self {
      cache_dir,
      config,
      native_config,
      modes,
      memory: RenderedImageMemoryCache::new(
        raw_memory_max_bytes,
        compressed_memory_max_bytes,
        memory_compression,
      ),
      last_success: HashMap::new(),
      failures: HashMap::new(),
      in_flight: HashSet::new(),
      in_flight_slots: HashMap::new(),
      visible_render_waits: HashSet::new(),
      priorities: HashMap::new(),
      jobs: HashMap::new(),
      active: HashSet::new(),
      active_preloads: 0,
      pending: BinaryHeap::new(),
      sequence: 0,
      prepared_images: Arc::new(Mutex::new(PreparedImageMemoryCache::new(
        prepared_memory_max_bytes,
      ))),
      max_concurrent,
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
    self.request_with_priority(page, width, height, kind, tx, RenderJobPriority::Visible)
  }

  pub fn preload(
    &mut self,
    page: &PageImage,
    width: u16,
    height: u16,
    kind: RenderKind,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) {
    if self.max_preloads() == 0 {
      debug!(
        page = page.page_index + 1,
        width,
        height,
        ?kind,
        max_concurrent = self.max_concurrent,
        "render preload skipped because no preload slots are available"
      );
      return;
    };
    self.request_with_priority(
      page,
      width,
      height,
      kind,
      tx,
      RenderJobPriority::TerminalPreload,
    );
  }

  fn request_with_priority(
    &mut self,
    page: &PageImage,
    width: u16,
    height: u16,
    kind: RenderKind,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
    priority: RenderJobPriority,
  ) -> RenderRequest {
    let cache_key = self.cache_key(page, width, height, kind);
    let slot_key = self.slot_key(page, width, height, kind);
    let preload = priority.is_preload();
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
        self.promote_render(cache_key.clone(), tx);
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
    if let Some(in_flight_key) = self.in_flight_slots.get(&slot_key).cloned()
      && (preload || in_flight_key == cache_key)
    {
      if !preload {
        self.visible_render_waits.insert(in_flight_key.clone());
        self.promote_render(in_flight_key.clone(), tx);
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
    self.priorities.insert(cache_key.clone(), priority);
    self.jobs.insert(
      cache_key.clone(),
      RenderJobState {
        page: page.clone(),
        width,
        height,
        kind,
        slot_key: slot_key.clone(),
      },
    );
    self.push_pending(cache_key.clone(), priority);
    debug!(
      page = page.page_index + 1,
      width,
      height,
      ?kind,
      cache_key = %cache_key,
      slot_key = %slot_key,
      preload,
      pending = self.pending.len(),
      "queued render request"
    );
    self.schedule_pending(tx);
    RenderRequest {
      cache_key,
      slot_key,
    }
  }

  fn push_pending(&mut self, cache_key: String, priority: RenderJobPriority) {
    let sequence = self.sequence;
    self.sequence = self.sequence.wrapping_add(1);
    self.pending.push(PendingRenderJob {
      priority,
      sequence,
      cache_key,
    });
  }

  fn promote_render(&mut self, cache_key: String, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    if self
      .priorities
      .get(&cache_key)
      .is_some_and(|priority| *priority < RenderJobPriority::Visible)
    {
      self
        .priorities
        .insert(cache_key.clone(), RenderJobPriority::Visible);
      self.push_pending(cache_key, RenderJobPriority::Visible);
      self.schedule_pending(tx);
    }
  }

  fn max_preloads(&self) -> usize {
    self.max_concurrent.saturating_sub(1)
  }

  fn schedule_pending(&mut self, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    while self.active.len() < self.max_concurrent {
      let Some(job) = self.pending.pop() else {
        break;
      };
      if !self.in_flight.contains(&job.cache_key) || self.active.contains(&job.cache_key) {
        continue;
      }
      let Some(priority) = self.priorities.get(&job.cache_key).copied() else {
        continue;
      };
      if job.priority < priority {
        continue;
      }
      if priority.is_preload() && self.active_preloads >= self.max_preloads() {
        self.pending.push(job);
        break;
      }
      self.spawn_render_job(job.cache_key, priority, tx);
    }
  }

  fn spawn_render_job(
    &mut self,
    cache_key: String,
    priority: RenderJobPriority,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) {
    let Some(job) = self.jobs.get(&cache_key).cloned() else {
      return;
    };
    let preload = priority.is_preload();
    self.active.insert(cache_key.clone());
    if preload {
      self.active_preloads = self.active_preloads.saturating_add(1);
    }
    let cache_dir = self.cache_dir.clone();
    let page = job.page;
    let width = job.width;
    let height = job.height;
    let kind = job.kind;
    let config = self.config.clone();
    let native_config = self.native_config.clone();
    let modes = self.modes.clone();
    let prepared_images = self.prepared_images.clone();
    let tx = tx.clone();
    let outcome_key = cache_key.clone();
    let outcome_slot_key = job.slot_key.clone();
    debug!(
      page = page.page_index + 1,
      width,
      height,
      ?kind,
      cache_key = %cache_key,
      slot_key = %outcome_slot_key,
      preload,
      active = self.active.len(),
      active_preloads = self.active_preloads,
      pending = self.pending.len(),
      "started render request"
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
      )
      .await;
      let _ = tx.send(AsyncEvent::Render(RenderOutcome {
        cache_key: outcome_key,
        slot_key: outcome_slot_key,
        preload,
        result,
      }));
    });
  }

  pub fn get(&mut self, cache_key: &str) -> Option<&RenderedImage> {
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
    self.priorities.clear();
    self.jobs.clear();
    self.active.clear();
    self.active_preloads = 0;
    self.pending.clear();
    self.sequence = 0;
    self.prepared_images = Arc::new(Mutex::new(PreparedImageMemoryCache::new(
      memory_cache_bytes(self.config.prepared_memory_cache_max_bytes),
    )));
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
      .filter(|fallback_key| self.memory.contains_key(fallback_key))
      .cloned()
  }

  pub fn mark_drawn(&mut self, cache_key: &str) {
    self.memory.touch(cache_key);
  }

  pub fn take_protocol_writes(
    &mut self,
    _drawn_render_keys: &[String],
    _include_background: bool,
  ) -> Vec<String> {
    Vec::new()
  }

  pub fn finish(
    &mut self,
    outcome: RenderOutcome,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) -> RenderFinish {
    self.in_flight.remove(&outcome.cache_key);
    self.priorities.remove(&outcome.cache_key);
    self.jobs.remove(&outcome.cache_key);
    if self.active.remove(&outcome.cache_key) && outcome.preload {
      self.active_preloads = self.active_preloads.saturating_sub(1);
    }
    let visible_wait = self.visible_render_waits.remove(&outcome.cache_key);
    if self
      .in_flight_slots
      .get(&outcome.slot_key)
      .is_some_and(|cache_key| cache_key == &outcome.cache_key)
    {
      self.in_flight_slots.remove(&outcome.slot_key);
    }
    let finish = match outcome.result {
      Ok(rendered) => {
        debug!(
          cache_key = %outcome.cache_key,
          slot_key = %outcome.slot_key,
          preload = outcome.preload,
          visible_wait,
          active = self.active.len(),
          active_preloads = self.active_preloads,
          pending = self.pending.len(),
          "render finish success"
        );
        self.failures.remove(&outcome.cache_key);
        self
          .last_success
          .insert(outcome.slot_key, outcome.cache_key.clone());
        let evicted = self.memory.insert(outcome.cache_key, rendered);
        self.drop_evicted_render_keys(&evicted);
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
          active = self.active.len(),
          active_preloads = self.active_preloads,
          pending = self.pending.len(),
          %error,
          "render finish error"
        );
        if outcome.preload {
          RenderFinish {
            message: None,
            needs_draw: visible_wait,
          }
        } else {
          self
            .failures
            .insert(outcome.cache_key.clone(), error.clone());
          RenderFinish {
            message: Some(format!("render failed: {error}")),
            needs_draw: true,
          }
        }
      }
    };
    self.schedule_pending(tx);
    finish
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

  fn drop_evicted_render_keys(&mut self, evicted: &[String]) {
    if evicted.is_empty() {
      return;
    }
    self
      .last_success
      .retain(|_, cache_key| !evicted.iter().any(|evicted| evicted == cache_key));
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
