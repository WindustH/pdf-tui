use std::collections::{BinaryHeap, HashMap, HashSet};

use tokio::sync::mpsc;
use tracing::debug;

use crate::event::{AsyncEvent, PageOutcome};

use super::{
  document::{PageSliceSpec, PdfDocument},
  raster::{
    preload_page_image, preload_page_slice_image, render_page_image, render_page_slice_image,
  },
};

pub struct PageStore {
  document: PdfDocument,
  in_flight: HashSet<PageRequestKey>,
  visible_waits: HashSet<PageRequestKey>,
  completed: HashMap<usize, PageRequestKey>,
  slice_in_flight: HashSet<PageSliceRequestKey>,
  slice_visible_waits: HashSet<PageSliceRequestKey>,
  slice_completed: HashSet<PageSliceRequestKey>,
  page_priorities: HashMap<PageRequestKey, PageJobPriority>,
  slice_priorities: HashMap<PageSliceRequestKey, PageJobPriority>,
  active_pages: HashSet<PageRequestKey>,
  active_slices: HashSet<PageSliceRequestKey>,
  active_preloads: usize,
  pending: BinaryHeap<PendingPageJob>,
  sequence: u64,
  max_concurrent: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct PageRequestKey {
  pub(super) page_index: usize,
  pub(super) target_width: u32,
  pub(super) target_height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PageSliceRequestKey {
  spec: PageSliceSpec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PageJobPriority {
  PagePreload,
  SlicePreload,
  Visible,
}

impl PageJobPriority {
  fn is_preload(self) -> bool {
    !matches!(self, Self::Visible)
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingPageJobKind {
  Page(PageRequestKey),
  Slice(PageSliceRequestKey),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingPageJob {
  priority: PageJobPriority,
  sequence: u64,
  kind: PendingPageJobKind,
}

impl Ord for PendingPageJob {
  fn cmp(&self, other: &Self) -> std::cmp::Ordering {
    self
      .priority
      .cmp(&other.priority)
      .then_with(|| other.sequence.cmp(&self.sequence))
  }
}

impl PartialOrd for PendingPageJob {
  fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
    Some(self.cmp(other))
  }
}

impl PageStore {
  pub fn new(document: PdfDocument, max_concurrent: usize) -> Self {
    let max_concurrent = max_concurrent.max(1);
    Self {
      document,
      in_flight: HashSet::new(),
      visible_waits: HashSet::new(),
      completed: HashMap::new(),
      slice_in_flight: HashSet::new(),
      slice_visible_waits: HashSet::new(),
      slice_completed: HashSet::new(),
      page_priorities: HashMap::new(),
      slice_priorities: HashMap::new(),
      active_pages: HashSet::new(),
      active_slices: HashSet::new(),
      active_preloads: 0,
      pending: BinaryHeap::new(),
      sequence: 0,
      max_concurrent,
    }
  }

  pub fn clear_state(&mut self) {
    self.in_flight.clear();
    self.visible_waits.clear();
    self.completed.clear();
    self.slice_in_flight.clear();
    self.slice_visible_waits.clear();
    self.slice_completed.clear();
    self.page_priorities.clear();
    self.slice_priorities.clear();
    self.active_pages.clear();
    self.active_slices.clear();
    self.active_preloads = 0;
    self.pending.clear();
    self.sequence = 0;
  }

  pub fn replace_document(&mut self, document: PdfDocument) {
    self.document = document;
    self.clear_state();
  }

  pub fn request_slice(&mut self, spec: PageSliceSpec, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    self.request_slice_with_priority(spec, tx, PageJobPriority::Visible);
  }

  pub fn preload_slice(&mut self, spec: PageSliceSpec, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    if self.max_preloads() == 0 {
      debug!(
        page = spec.page_index + 1,
        slice = spec.slice_index + 1,
        slice_count = spec.slice_count,
        max_concurrent = self.max_concurrent,
        "page slice preload skipped because no preload slots are available"
      );
      return;
    };
    self.request_slice_with_priority(spec, tx, PageJobPriority::SlicePreload);
  }

  fn request_slice_with_priority(
    &mut self,
    spec: PageSliceSpec,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
    priority: PageJobPriority,
  ) {
    let spec = spec.normalized();
    let key = PageSliceRequestKey { spec };
    let preload = priority.is_preload();
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
        self.promote_slice(key, tx);
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
    self.slice_priorities.insert(key, priority);
    self.push_pending(PendingPageJobKind::Slice(key), priority);
    debug!(
      page = spec.page_index + 1,
      slice = spec.slice_index + 1,
      slice_count = spec.slice_count,
      target_width = spec.target_width,
      target_height = spec.target_height,
      slice_y = spec.slice_y,
      slice_height = spec.slice_height,
      preload,
      pending = self.pending.len(),
      "queued page slice render request"
    );
    self.schedule_pending(tx);
  }

  pub fn request(
    &mut self,
    page_index: usize,
    target_width: u32,
    target_height: u32,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) {
    self.request_with_priority(
      page_index,
      target_width,
      target_height,
      tx,
      PageJobPriority::Visible,
    );
  }

  pub fn preload(
    &mut self,
    page_index: usize,
    target_width: u32,
    target_height: u32,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) {
    if self.max_preloads() == 0 {
      debug!(
        page = page_index + 1,
        max_concurrent = self.max_concurrent,
        "page preload skipped because no preload slots are available"
      );
      return;
    };
    self.request_with_priority(
      page_index,
      target_width,
      target_height,
      tx,
      PageJobPriority::PagePreload,
    );
  }

  fn request_with_priority(
    &mut self,
    page_index: usize,
    target_width: u32,
    target_height: u32,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
    priority: PageJobPriority,
  ) {
    let key = PageRequestKey {
      page_index,
      target_width: target_width.max(1),
      target_height: target_height.max(1),
    };
    let preload = priority.is_preload();
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
        self.promote_page(key, tx);
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
    self.page_priorities.insert(key, priority);
    self.push_pending(PendingPageJobKind::Page(key), priority);
    debug!(
      page = page_index + 1,
      target_width = key.target_width,
      target_height = key.target_height,
      preload,
      pending = self.pending.len(),
      "queued page render request"
    );
    self.schedule_pending(tx);
  }

  fn push_pending(&mut self, kind: PendingPageJobKind, priority: PageJobPriority) {
    let sequence = self.sequence;
    self.sequence = self.sequence.wrapping_add(1);
    self.pending.push(PendingPageJob {
      priority,
      sequence,
      kind,
    });
  }

  fn promote_page(&mut self, key: PageRequestKey, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    if self
      .page_priorities
      .get(&key)
      .is_some_and(|priority| *priority < PageJobPriority::Visible)
    {
      self.page_priorities.insert(key, PageJobPriority::Visible);
      self.push_pending(PendingPageJobKind::Page(key), PageJobPriority::Visible);
      self.schedule_pending(tx);
    }
  }

  fn promote_slice(&mut self, key: PageSliceRequestKey, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    if self
      .slice_priorities
      .get(&key)
      .is_some_and(|priority| *priority < PageJobPriority::Visible)
    {
      self.slice_priorities.insert(key, PageJobPriority::Visible);
      self.push_pending(PendingPageJobKind::Slice(key), PageJobPriority::Visible);
      self.schedule_pending(tx);
    }
  }

  fn active_count(&self) -> usize {
    self.active_pages.len() + self.active_slices.len()
  }

  fn max_preloads(&self) -> usize {
    self.max_concurrent.saturating_sub(1)
  }

  fn schedule_pending(&mut self, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    while self.active_count() < self.max_concurrent {
      let Some(job) = self.pending.pop() else {
        break;
      };
      match job.kind {
        PendingPageJobKind::Page(key) => {
          if !self.in_flight.contains(&key) || self.active_pages.contains(&key) {
            continue;
          }
          let Some(priority) = self.page_priorities.get(&key).copied() else {
            continue;
          };
          if job.priority < priority {
            continue;
          }
          if priority.is_preload() && self.active_preloads >= self.max_preloads() {
            self.pending.push(job);
            break;
          }
          self.spawn_page_job(key, priority, tx);
        }
        PendingPageJobKind::Slice(key) => {
          if !self.slice_in_flight.contains(&key) || self.active_slices.contains(&key) {
            continue;
          }
          let Some(priority) = self.slice_priorities.get(&key).copied() else {
            continue;
          };
          if job.priority < priority {
            continue;
          }
          if priority.is_preload() && self.active_preloads >= self.max_preloads() {
            self.pending.push(job);
            break;
          }
          self.spawn_slice_job(key, priority, tx);
        }
      }
    }
  }

  fn spawn_page_job(
    &mut self,
    key: PageRequestKey,
    priority: PageJobPriority,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) {
    let preload = priority.is_preload();
    self.active_pages.insert(key);
    if preload {
      self.active_preloads = self.active_preloads.saturating_add(1);
    }
    debug!(
      page = key.page_index + 1,
      target_width = key.target_width,
      target_height = key.target_height,
      preload,
      active = self.active_count(),
      active_preloads = self.active_preloads,
      pending = self.pending.len(),
      "started page render request"
    );
    let document = self.document.clone();
    let source_size_bytes = document.size_bytes;
    let source_modified_nanos = document.modified_nanos;
    let tx = tx.clone();
    tokio::spawn(async move {
      let result = if preload {
        preload_page_image(&document, key).await
      } else {
        render_page_image(&document, key).await
      }
      .map_err(|error| error.to_string());
      let _ = tx.send(AsyncEvent::Page(PageOutcome {
        source_size_bytes,
        source_modified_nanos,
        page_index: key.page_index,
        target_width: key.target_width,
        target_height: key.target_height,
        slice: None,
        preload,
        result,
      }));
    });
  }

  fn spawn_slice_job(
    &mut self,
    key: PageSliceRequestKey,
    priority: PageJobPriority,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) {
    let spec = key.spec;
    let preload = priority.is_preload();
    self.active_slices.insert(key);
    if preload {
      self.active_preloads = self.active_preloads.saturating_add(1);
    }
    debug!(
      page = spec.page_index + 1,
      slice = spec.slice_index + 1,
      slice_count = spec.slice_count,
      target_width = spec.target_width,
      target_height = spec.target_height,
      preload,
      active = self.active_count(),
      active_preloads = self.active_preloads,
      pending = self.pending.len(),
      "started page slice render request"
    );
    let document = self.document.clone();
    let source_size_bytes = document.size_bytes;
    let source_modified_nanos = document.modified_nanos;
    let tx = tx.clone();
    tokio::spawn(async move {
      let result = if preload {
        preload_page_slice_image(&document, spec).await
      } else {
        render_page_slice_image(&document, spec).await
      }
      .map_err(|error| error.to_string());
      let _ = tx.send(AsyncEvent::Page(PageOutcome {
        source_size_bytes,
        source_modified_nanos,
        page_index: spec.page_index,
        target_width: spec.target_width,
        target_height: spec.target_height,
        slice: Some(spec),
        preload,
        result,
      }));
    });
  }

  pub fn finish(
    &mut self,
    page: &PageOutcome,
    completed: bool,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) -> bool {
    if let Some(spec) = page.slice {
      let slice_id = spec.id();
      let key = PageSliceRequestKey { spec };
      self.slice_in_flight.remove(&key);
      self.slice_priorities.remove(&key);
      if self.active_slices.remove(&key) && page.preload {
        self.active_preloads = self.active_preloads.saturating_sub(1);
      }
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
        active = self.active_count(),
        active_preloads = self.active_preloads,
        pending = self.pending.len(),
        "page slice store finish"
      );
      self.schedule_pending(tx);
      return visible_wait;
    }

    let key = PageRequestKey {
      page_index: page.page_index,
      target_width: page.target_width.max(1),
      target_height: page.target_height,
    };
    self.in_flight.remove(&key);
    self.page_priorities.remove(&key);
    if self.active_pages.remove(&key) && page.preload {
      self.active_preloads = self.active_preloads.saturating_sub(1);
    }
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
      active = self.active_count(),
      active_preloads = self.active_preloads,
      pending = self.pending.len(),
      "page store finish"
    );
    self.schedule_pending(tx);
    visible_wait
  }
}
