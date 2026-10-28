//! Scheduling of page rasterization: whole-page PNGs and scroll slices,
//! visible requests first, with results reported as `AsyncEvent::Page`.

use std::collections::{HashMap, HashSet};

use tokio::sync::mpsc;
use tracing::debug;

use crate::job_queue::{JobPriority, JobQueue};

use super::{
  document::{PageImage, PageSliceSpec, PdfDocument},
  raster::{
    preload_page_image, preload_page_slice_image, render_page_image, render_page_slice_image,
  },
};

pub struct PageStore {
  document: PdfDocument,
  jobs: JobQueue<PageJob, PageJobPriority>,
  /// Queued or running jobs a visible request is waiting for; finishing one
  /// triggers a redraw even when it started as a preload.
  visible_waits: HashSet<PageJob>,
  /// Last finished whole-page request per page.
  completed: HashMap<usize, PageRequestKey>,
  completed_slices: HashSet<PageSliceSpec>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct PageRequestKey {
  pub(super) page_index: usize,
  pub(super) target_width: u32,
  pub(super) target_height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum PageJob {
  Page(PageRequestKey),
  Slice(PageSliceSpec),
}

impl PageJob {
  fn page_index(self) -> usize {
    match self {
      Self::Page(key) => key.page_index,
      Self::Slice(spec) => spec.page_index,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PageJobPriority {
  PagePreload,
  SlicePreload,
  Visible,
}

impl JobPriority for PageJobPriority {
  const VISIBLE: Self = Self::Visible;
}

impl PageStore {
  pub fn new(document: PdfDocument, max_concurrent: usize) -> Self {
    Self {
      document,
      jobs: JobQueue::new(max_concurrent),
      visible_waits: HashSet::new(),
      completed: HashMap::new(),
      completed_slices: HashSet::new(),
    }
  }

  pub fn clear_state(&mut self) {
    self.jobs.clear();
    self.visible_waits.clear();
    self.completed.clear();
    self.completed_slices.clear();
  }

  pub fn cancel_preloads(&mut self) {
    for job in self.jobs.cancel_queued_preloads() {
      self.visible_waits.remove(&job);
    }
  }

  pub fn replace_document(&mut self, document: PdfDocument) {
    self.document = document;
    self.clear_state();
  }

  pub fn request_slice<E: From<PageOutcome> + Send + 'static>(
    &mut self,
    spec: PageSliceSpec,
    tx: &mpsc::UnboundedSender<E>,
  ) {
    self.request_slice_with_priority(spec, PageJobPriority::Visible, tx);
  }

  pub fn preload_slice<E: From<PageOutcome> + Send + 'static>(
    &mut self,
    spec: PageSliceSpec,
    tx: &mpsc::UnboundedSender<E>,
  ) {
    if self.jobs.allows_preloads() {
      self.request_slice_with_priority(spec, PageJobPriority::SlicePreload, tx);
    }
  }

  pub fn request<E: From<PageOutcome> + Send + 'static>(
    &mut self,
    page_index: usize,
    target_width: u32,
    target_height: u32,
    tx: &mpsc::UnboundedSender<E>,
  ) {
    let key = page_key(page_index, target_width, target_height);
    self.request_page_with_priority(key, PageJobPriority::Visible, tx);
  }

  pub fn preload<E: From<PageOutcome> + Send + 'static>(
    &mut self,
    page_index: usize,
    target_width: u32,
    target_height: u32,
    tx: &mpsc::UnboundedSender<E>,
  ) {
    if self.jobs.allows_preloads() {
      let key = page_key(page_index, target_width, target_height);
      self.request_page_with_priority(key, PageJobPriority::PagePreload, tx);
    }
  }

  fn request_slice_with_priority<E: From<PageOutcome> + Send + 'static>(
    &mut self,
    spec: PageSliceSpec,
    priority: PageJobPriority,
    tx: &mpsc::UnboundedSender<E>,
  ) {
    let spec = spec.normalized();
    if self.completed_slices.contains(&spec) {
      return;
    }
    self.request_job(PageJob::Slice(spec), priority, tx);
  }

  fn request_page_with_priority<E: From<PageOutcome> + Send + 'static>(
    &mut self,
    key: PageRequestKey,
    priority: PageJobPriority,
    tx: &mpsc::UnboundedSender<E>,
  ) {
    if self.completed.get(&key.page_index) == Some(&key) {
      return;
    }
    self.request_job(PageJob::Page(key), priority, tx);
  }

  fn request_job<E: From<PageOutcome> + Send + 'static>(
    &mut self,
    job: PageJob,
    priority: PageJobPriority,
    tx: &mpsc::UnboundedSender<E>,
  ) {
    if job.page_index() >= self.document.page_count {
      return;
    }
    if self.jobs.contains(&job) {
      if !priority.is_preload() {
        self.visible_waits.insert(job);
        self.jobs.promote(&job, priority);
        self.schedule(tx);
      }
      return;
    }
    self.jobs.enqueue(job, priority);
    debug!(
      ?job,
      ?priority,
      queued = self.jobs.queued_count(),
      "queued page request"
    );
    self.schedule(tx);
  }

  fn schedule<E: From<PageOutcome> + Send + 'static>(&mut self, tx: &mpsc::UnboundedSender<E>) {
    while let Some((job, priority)) = self.jobs.start_next() {
      debug!(
        ?job,
        ?priority,
        running = self.jobs.running_count(),
        running_preloads = self.jobs.running_preloads(),
        queued = self.jobs.queued_count(),
        "started page request"
      );
      spawn_page_job(
        self.document.clone(),
        job,
        priority.is_preload(),
        tx.clone(),
      );
    }
  }

  /// Records a finished job; returns whether a visible request was waiting
  /// for it. `completed` marks it done so it is not requested again.
  pub fn finish<E: From<PageOutcome> + Send + 'static>(
    &mut self,
    page: &PageOutcome,
    completed: bool,
    tx: &mpsc::UnboundedSender<E>,
  ) -> bool {
    let job = match page.slice {
      Some(spec) => PageJob::Slice(spec),
      None => PageJob::Page(page_key(
        page.page_index,
        page.target_width,
        page.target_height,
      )),
    };
    self.jobs.finish(&job);
    let visible_wait = self.visible_waits.remove(&job);
    if completed {
      match job {
        PageJob::Page(key) => {
          self.completed.insert(key.page_index, key);
        }
        PageJob::Slice(spec) => {
          self.completed_slices.insert(spec);
        }
      }
    }
    debug!(
      ?job,
      completed,
      visible_wait,
      running = self.jobs.running_count(),
      queued = self.jobs.queued_count(),
      "page store finish"
    );
    self.schedule(tx);
    visible_wait
  }
}

fn page_key(page_index: usize, target_width: u32, target_height: u32) -> PageRequestKey {
  PageRequestKey {
    page_index,
    target_width: target_width.max(1),
    target_height: target_height.max(1),
  }
}

fn spawn_page_job<E: From<PageOutcome> + Send + 'static>(
  document: PdfDocument,
  job: PageJob,
  preload: bool,
  tx: mpsc::UnboundedSender<E>,
) {
  tokio::spawn(async move {
    let (result, slice, key) = match job {
      PageJob::Page(key) => {
        let result = if preload {
          preload_page_image(&document, key).await
        } else {
          render_page_image(&document, key).await
        };
        (result, None, key)
      }
      PageJob::Slice(spec) => {
        let result = if preload {
          preload_page_slice_image(&document, spec).await
        } else {
          render_page_slice_image(&document, spec).await
        };
        let key = page_key(spec.page_index, spec.target_width, spec.target_height);
        (result, Some(spec), key)
      }
    };
    let _ = tx.send(E::from(PageOutcome {
      source_size_bytes: document.size_bytes,
      source_modified_nanos: document.modified_nanos,
      page_index: key.page_index,
      target_width: key.target_width,
      target_height: key.target_height,
      slice,
      preload,
      result: result.map_err(|error| error.to_string()),
    }));
  });
}

#[derive(Debug)]
pub struct PageOutcome {
  pub source_size_bytes: u64,
  pub source_modified_nanos: u128,
  pub page_index: usize,
  pub target_width: u32,
  pub target_height: u32,
  pub slice: Option<PageSliceSpec>,
  pub preload: bool,
  pub result: Result<PageImage, String>,
}
