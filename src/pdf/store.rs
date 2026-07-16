use std::{
  collections::{HashMap, HashSet},
  sync::Arc,
};

use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
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
  max_concurrent: usize,
  semaphore: Arc<Semaphore>,
  preload_semaphore: Arc<Semaphore>,
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

struct PagePermits {
  _global: OwnedSemaphorePermit,
  _preload: Option<OwnedSemaphorePermit>,
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

  pub fn replace_document(&mut self, document: PdfDocument) {
    self.document = document;
    self.clear_state();
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
    let source_size_bytes = document.size_bytes;
    let source_modified_nanos = document.modified_nanos;
    let tx = tx.clone();
    let semaphore = self.semaphore.clone();
    tokio::spawn(async move {
      let result = match acquire_page_permits(semaphore, permits).await {
        Ok(permits) => {
          let _permits = permits;
          if preload {
            preload_page_slice_image(&document, spec).await
          } else {
            render_page_slice_image(&document, spec).await
          }
          .map_err(|error| error.to_string())
        }
        Err(error) => Err(error),
      };
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
    let source_size_bytes = document.size_bytes;
    let source_modified_nanos = document.modified_nanos;
    let tx = tx.clone();
    let semaphore = self.semaphore.clone();
    tokio::spawn(async move {
      let result = match acquire_page_permits(semaphore, permits).await {
        Ok(permits) => {
          let _permits = permits;
          if preload {
            preload_page_image(&document, key).await
          } else {
            render_page_image(&document, key).await
          }
          .map_err(|error| error.to_string())
        }
        Err(error) => Err(error),
      };
      let _ = tx.send(AsyncEvent::Page(PageOutcome {
        source_size_bytes,
        source_modified_nanos,
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
