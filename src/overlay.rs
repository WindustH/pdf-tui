//! Page images with search highlights and selection marks drawn in.
//!
//! Producing one decodes and re-encodes a PNG, so it runs on the blocking
//! pool and the result is remembered by request; the draw path only looks
//! results up and shows the plain image until the marked one is ready.

use std::{
  collections::{HashMap, HashSet},
  hash::{DefaultHasher, Hash, Hasher},
  path::PathBuf,
};

use tokio::sync::mpsc;
use tracing::debug;

use crate::{
  config::RenderConfig,
  event::{AsyncEvent, OverlayOutcome},
  pdf::PageImage,
  search::{self, PdfSearchMatch},
  selection::{self, PdfRect, PdfSelection},
};

/// Remembered results beyond this are dropped wholesale; they are cached on
/// disk, so a later request is cheap.
const MAX_READY: usize = 256;

/// One mark applied on top of the previous image.
#[derive(Debug, Clone)]
pub enum OverlayStep {
  /// Inverts a search match; skipped when it lies outside the image.
  SearchHighlight(PdfSearchMatch),
  /// Selection rectangle outline on a page image of `page_size` points.
  PageOutline {
    page_size: (u32, u32),
    rect: PdfRect,
  },
  /// Selection anchor crosshair on a page image.
  PageMarker {
    page_size: (u32, u32),
    rect: PdfRect,
  },
  /// Outline on an image cropped to `selection`.
  CropOutline {
    selection: PdfSelection,
    rect: PdfRect,
  },
  /// Anchor crosshair on an image cropped to `selection`.
  CropMarker {
    selection: PdfSelection,
    rect: PdfRect,
  },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OverlayKey(u64);

pub enum OverlayState {
  Ready(PageImage),
  Failed(String),
  Pending,
}

pub struct OverlayStore {
  cache_dir: PathBuf,
  search_highlight_max_bytes: u64,
  selection_max_bytes: u64,
  ready: HashMap<OverlayKey, Result<PageImage, String>>,
  in_flight: HashSet<OverlayKey>,
}

impl OverlayStore {
  pub fn new(cache_dir: PathBuf, render: &RenderConfig) -> Self {
    Self {
      cache_dir,
      search_highlight_max_bytes: render.search_highlight_cache_max_bytes,
      selection_max_bytes: render.selection_cache_max_bytes,
      ready: HashMap::new(),
      in_flight: HashSet::new(),
    }
  }

  /// `base` with `steps` applied, starting the work when it is not known
  /// yet. `preload` requests do not trigger a redraw when they finish.
  pub fn request(
    &mut self,
    base: &PageImage,
    steps: &[OverlayStep],
    preload: bool,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) -> OverlayState {
    let key = overlay_key(base, steps);
    match self.ready.get(&key) {
      Some(Ok(image)) => OverlayState::Ready(image.clone()),
      Some(Err(error)) => OverlayState::Failed(error.clone()),
      None => {
        if self.in_flight.insert(key) {
          self.spawn(key, base.clone(), steps.to_vec(), preload, tx);
        }
        OverlayState::Pending
      }
    }
  }

  /// Stores a finished job; returns whether it was still wanted.
  pub fn finish(&mut self, outcome: OverlayOutcome) -> bool {
    if !self.in_flight.remove(&outcome.key) {
      return false;
    }
    if self.ready.len() >= MAX_READY {
      self.ready.clear();
    }
    self.ready.insert(outcome.key, outcome.result);
    true
  }

  /// Forgets everything, e.g. after a reload or a cache clear; jobs still
  /// running are ignored when they finish.
  pub fn clear(&mut self) {
    self.ready.clear();
    self.in_flight.clear();
  }

  fn spawn(
    &self,
    key: OverlayKey,
    base: PageImage,
    steps: Vec<OverlayStep>,
    preload: bool,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) {
    let job = OverlayJob {
      cache_dir: self.cache_dir.clone(),
      search_highlight_max_bytes: self.search_highlight_max_bytes,
      selection_max_bytes: self.selection_max_bytes,
    };
    let tx = tx.clone();
    debug!(
      page = base.page_index + 1,
      steps = steps.len(),
      preload,
      "overlay requested"
    );
    tokio::task::spawn_blocking(move || {
      let result = job.apply(base, &steps);
      let _ = tx.send(AsyncEvent::Overlay(OverlayOutcome {
        key,
        preload,
        result,
      }));
    });
  }
}

struct OverlayJob {
  cache_dir: PathBuf,
  search_highlight_max_bytes: u64,
  selection_max_bytes: u64,
}

impl OverlayJob {
  fn apply(&self, base: PageImage, steps: &[OverlayStep]) -> Result<PageImage, String> {
    let cache_dir = &self.cache_dir;
    let max_bytes = self.selection_max_bytes;
    steps.iter().try_fold(base, |image, step| match step {
      OverlayStep::SearchHighlight(found) => Ok(
        search::highlighted_viewer_image(
          cache_dir,
          &image,
          found,
          self.search_highlight_max_bytes,
        )?
        .unwrap_or(image),
      ),
      OverlayStep::PageOutline { page_size, rect } => {
        selection::outline_page_image(cache_dir, &image, *page_size, *rect, max_bytes)
      }
      OverlayStep::PageMarker { page_size, rect } => {
        selection::marker_page_image(cache_dir, &image, *page_size, *rect, max_bytes)
      }
      OverlayStep::CropOutline { selection, rect } => {
        selection::outline_selection_crop_image(cache_dir, &image, *selection, *rect, max_bytes)
      }
      OverlayStep::CropMarker { selection, rect } => {
        selection::marker_selection_crop_image(cache_dir, &image, *selection, *rect, max_bytes)
      }
    })
  }
}

fn overlay_key(base: &PageImage, steps: &[OverlayStep]) -> OverlayKey {
  let mut hasher = DefaultHasher::new();
  base.path.hash(&mut hasher);
  base.size_bytes.hash(&mut hasher);
  base.modified_nanos.hash(&mut hasher);
  (base.width, base.height).hash(&mut hasher);
  base
    .slice
    .as_ref()
    .map(|slice| &slice.cache_key)
    .hash(&mut hasher);
  for step in steps {
    hash_step(step, &mut hasher);
  }
  OverlayKey(hasher.finish())
}

fn hash_step(step: &OverlayStep, hasher: &mut DefaultHasher) {
  let hash_rect = |rect: &PdfRect, hasher: &mut DefaultHasher| {
    for value in [rect.x_min, rect.y_min, rect.x_max, rect.y_max] {
      value.to_bits().hash(hasher);
    }
  };
  let hash_selection = |selection: &PdfSelection, hasher: &mut DefaultHasher| {
    selection.page_index.hash(hasher);
    selection.page_width.to_bits().hash(hasher);
    selection.page_height.to_bits().hash(hasher);
    hash_rect(&selection.rect, hasher);
  };
  std::mem::discriminant(step).hash(hasher);
  match step {
    OverlayStep::SearchHighlight(found) => {
      found.page_index.hash(hasher);
      for value in [
        found.rect.x_min,
        found.rect.y_min,
        found.rect.x_max,
        found.rect.y_max,
        found.page_width,
        found.page_height,
      ] {
        value.to_bits().hash(hasher);
      }
    }
    OverlayStep::PageOutline { page_size, rect } | OverlayStep::PageMarker { page_size, rect } => {
      page_size.hash(hasher);
      hash_rect(rect, hasher);
    }
    OverlayStep::CropOutline { selection, rect } | OverlayStep::CropMarker { selection, rect } => {
      hash_selection(selection, hasher);
      hash_rect(rect, hasher);
    }
  }
}
