mod cache_file;
mod chafa;
mod driver;
mod key;
mod memory;

use std::{
  collections::{HashMap, HashSet},
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
  job_queue::{JobPriority, JobQueue},
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

/// Turns page PNGs into terminal output (protocol escape sequences or
/// Chafa text) for a cell area, trying the configured render modes in
/// order, and keeps the results in memory and on disk.
pub struct RenderStore {
  cache_dir: PathBuf,
  config: RenderConfig,
  native_config: NativeImageConfig,
  modes: Vec<RenderMode>,
  memory: RenderedImageMemoryCache,
  /// Failed visible renders; they are not retried until the state is
  /// cleared.
  failures: HashMap<String, String>,
  jobs: JobQueue<String, RenderJobPriority>,
  /// Inputs of queued or running jobs.
  job_inputs: HashMap<String, RenderJobInput>,
  /// Queued or running jobs a visible request is waiting for.
  visible_waits: HashSet<String>,
  prepared_images: PreparedImageCache,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RenderJobPriority {
  TerminalPreload,
  Visible,
}

impl JobPriority for RenderJobPriority {
  const VISIBLE: Self = Self::Visible;
}

#[derive(Clone)]
struct RenderJobInput {
  page: PageImage,
  width: u16,
  height: u16,
  kind: RenderKind,
}

impl RenderStore {
  pub fn new(
    cache_dir: PathBuf,
    config: RenderConfig,
    native_config: NativeImageConfig,
    modes: Vec<RenderMode>,
  ) -> Self {
    let raw_memory_max_bytes = memory_cache_bytes(config.raw_memory_cache_max_bytes);
    let compressed_memory_max_bytes = memory_cache_bytes(config.compressed_memory_cache_max_bytes);
    let memory_compression = config.memory_compression;
    Self {
      cache_dir,
      memory: RenderedImageMemoryCache::new(
        raw_memory_max_bytes,
        compressed_memory_max_bytes,
        memory_compression,
      ),
      failures: HashMap::new(),
      jobs: JobQueue::new(config.max_concurrent),
      job_inputs: HashMap::new(),
      visible_waits: HashSet::new(),
      prepared_images: new_prepared_cache(&config),
      config,
      native_config,
      modes,
    }
  }

  /// Requests `page` rendered into `width` x `height` cells; returns the
  /// key under which the result becomes available.
  pub fn request(
    &mut self,
    page: &PageImage,
    width: u16,
    height: u16,
    kind: RenderKind,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) -> String {
    self.request_with_priority(page, width, height, kind, RenderJobPriority::Visible, tx)
  }

  pub fn preload(
    &mut self,
    page: &PageImage,
    width: u16,
    height: u16,
    kind: RenderKind,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) {
    if self.jobs.allows_preloads() {
      self.request_with_priority(
        page,
        width,
        height,
        kind,
        RenderJobPriority::TerminalPreload,
        tx,
      );
    }
  }

  fn request_with_priority(
    &mut self,
    page: &PageImage,
    width: u16,
    height: u16,
    kind: RenderKind,
    priority: RenderJobPriority,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) -> String {
    let cache_key = self.cache_key(page, width, height, kind);
    if width == 0 || height == 0 || self.memory.contains_key(&cache_key) {
      return cache_key;
    }
    let preload = priority.is_preload();
    if self.jobs.contains(&cache_key) {
      if !preload {
        self.visible_waits.insert(cache_key.clone());
        self.jobs.promote(&cache_key, priority);
        self.schedule(tx);
      }
      return cache_key;
    }
    if !preload && self.failures.contains_key(&cache_key) {
      return cache_key;
    }
    self.job_inputs.insert(
      cache_key.clone(),
      RenderJobInput {
        page: page.clone(),
        width,
        height,
        kind,
      },
    );
    self.jobs.enqueue(cache_key.clone(), priority);
    debug!(
      page = page.page_index + 1,
      width,
      height,
      cache_key = %cache_key,
      preload,
      queued = self.jobs.queued_count(),
      "queued render request"
    );
    self.schedule(tx);
    cache_key
  }

  fn schedule(&mut self, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    while let Some((cache_key, priority)) = self.jobs.start_next() {
      let Some(input) = self.job_inputs.get(&cache_key).cloned() else {
        self.jobs.finish(&cache_key);
        continue;
      };
      let preload = priority.is_preload();
      debug!(
        page = input.page.page_index + 1,
        width = input.width,
        height = input.height,
        cache_key = %cache_key,
        preload,
        running = self.jobs.running_count(),
        running_preloads = self.jobs.running_preloads(),
        queued = self.jobs.queued_count(),
        "started render request"
      );
      let cache_dir = self.cache_dir.clone();
      let config = self.config.clone();
      let native_config = self.native_config.clone();
      let modes = self.modes.clone();
      let prepared_images = self.prepared_images.clone();
      let tx = tx.clone();
      tokio::spawn(async move {
        let result = render_with_fallbacks(
          input.page,
          input.width,
          input.height,
          input.kind,
          cache_dir,
          config,
          native_config,
          modes,
          prepared_images,
        )
        .await;
        let _ = tx.send(AsyncEvent::Render(RenderOutcome {
          cache_key,
          preload,
          result,
        }));
      });
    }
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
    self.failures.clear();
    self.jobs.clear();
    self.job_inputs.clear();
    self.visible_waits.clear();
    self.prepared_images = new_prepared_cache(&self.config);
  }

  pub fn cancel_preloads(&mut self) {
    for cache_key in self.jobs.cancel_queued_preloads() {
      self.visible_waits.remove(&cache_key);
      self.job_inputs.remove(&cache_key);
    }
  }

  pub fn mark_drawn(&mut self, cache_key: &str) {
    self.memory.touch(cache_key);
  }

  pub fn finish(
    &mut self,
    outcome: RenderOutcome,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) -> RenderFinish {
    self.jobs.finish(&outcome.cache_key);
    self.job_inputs.remove(&outcome.cache_key);
    let visible_wait = self.visible_waits.remove(&outcome.cache_key);
    let finish = match outcome.result {
      Ok(rendered) => {
        self.failures.remove(&outcome.cache_key);
        self.memory.insert(outcome.cache_key, rendered);
        RenderFinish {
          message: None,
          needs_draw: !outcome.preload || visible_wait,
        }
      }
      // A failed preload is retried when the page becomes visible.
      Err(_) if outcome.preload => RenderFinish {
        message: None,
        needs_draw: visible_wait,
      },
      Err(error) => {
        let message = Some(format!("render failed: {error}"));
        self.failures.insert(outcome.cache_key, error);
        RenderFinish {
          message,
          needs_draw: true,
        }
      }
    };
    self.schedule(tx);
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
    hash_render_kind(&mut hasher, kind);
    hash_render_config(&mut hasher, &self.config);
    hash_native_config(&mut hasher, &self.native_config);
    for mode in &self.modes {
      hasher.update(mode.label().as_bytes());
      hasher.update([0]);
    }
    hex::encode(hasher.finalize())
  }
}

fn new_prepared_cache(config: &RenderConfig) -> PreparedImageCache {
  Arc::new(Mutex::new(PreparedImageMemoryCache::new(
    memory_cache_bytes(config.prepared_memory_cache_max_bytes),
  )))
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
