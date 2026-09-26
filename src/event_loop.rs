//! The main loop: draws frames, runs the external editor, and applies
//! async events (input, finished renders, reloads, ...) to the app state.

use anyhow::Result;
use framework_tui::edit_text_in_editor;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::{
  app::{App, EditorRequest, ViewMode},
  background::{InputGate, discard_pending_terminal_events},
  event::{
    AsyncEvent, CacheClearOutcome, DocumentReload, PageOutcome, RenderOutcome, RenderedImage,
    SearchIndexOutcome, SelectionImageOutcome,
  },
  terminal::Tui,
  ui::{self, ImagePipeline},
};

pub struct Session {
  pub app: App,
  pipeline: ImagePipeline,
  tx: mpsc::UnboundedSender<AsyncEvent>,
  rx: mpsc::UnboundedReceiver<AsyncEvent>,
  input: InputGate,
}

impl Session {
  pub fn new(
    app: App,
    pipeline: ImagePipeline,
    tx: mpsc::UnboundedSender<AsyncEvent>,
    rx: mpsc::UnboundedReceiver<AsyncEvent>,
    input: InputGate,
  ) -> Self {
    Self {
      app,
      pipeline,
      tx,
      rx,
      input,
    }
  }

  /// Runs until the user quits or every event sender is gone. Events that
  /// arrive together are applied as a batch before the next frame.
  pub async fn run(&mut self, tui: &mut Tui) -> Result<()> {
    let mut needs_draw = true;
    loop {
      if needs_draw {
        debug!(
          scroll = self.app.scroll,
          focused_page = self.app.focused_page,
          layout = %self.app.layout.label(),
          "drawing frame"
        );
        tui.draw(|frame| ui::draw(frame, &mut self.app, &mut self.pipeline, &self.tx))?;
        needs_draw = false;
        if self.app.should_quit() {
          return Ok(());
        }
      }

      if let Some(request) = self.app.take_editor_request() {
        self.run_editor(tui, request)?;
        needs_draw = true;
        continue;
      }

      let Some(message) = self.rx.recv().await else {
        return Ok(());
      };
      needs_draw |= self.handle(message);
      while let Ok(message) = self.rx.try_recv() {
        needs_draw |= self.handle(message);
      }
    }
  }

  /// Hands the terminal to `$EDITOR` and feeds the edited text back.
  fn run_editor(&mut self, tui: &mut Tui, request: EditorRequest) -> Result<()> {
    self.input.pause();
    let suspended = tui.suspend();
    let result = suspended
      .as_ref()
      .map_err(ToString::to_string)
      .and_then(|_| edit_text_in_editor(request.initial_text(), &self.app.settings.cache_dir));
    let resumed = tui.resume();
    if resumed.is_ok() {
      discard_pending_terminal_events();
    }
    self.input.resume();
    match request {
      EditorRequest::Metadata { original, .. } => {
        self.app.finish_metadata_editor_input(original, result)
      }
      EditorRequest::Bookmarks { original, .. } => {
        self.app.finish_bookmarks_editor_input(original, result)
      }
    }
    suspended?;
    resumed
  }

  /// Applies one event; returns whether the screen needs a redraw.
  fn handle(&mut self, message: AsyncEvent) -> bool {
    match message {
      AsyncEvent::Input { event, generation } => self.on_input(event, generation),
      AsyncEvent::Page(outcome) => self.on_page(outcome),
      AsyncEvent::Render(outcome) => self.on_render(outcome),
      AsyncEvent::AutoRefreshRequested => {
        self.app.request_refresh(&self.tx);
        true
      }
      AsyncEvent::Refresh(outcome) => {
        let queued = self.app.finish_refresh_request();
        match outcome.result {
          Ok(reload) => {
            self.apply_document_reload(reload);
            self.app.set_message("refreshed current document");
            info!(path = %self.app.document.path.display(), "document refreshed");
          }
          Err(error) => {
            self.app.set_message(format!("refresh failed: {error}"));
            warn!(%error, "document refresh failed");
          }
        }
        if queued {
          self.app.request_refresh(&self.tx);
        }
        true
      }
      AsyncEvent::MetadataWrite(outcome) => self.on_document_write(
        outcome.result,
        format!("metadata updated: {} tag(s)", outcome.changed_tags),
        "metadata write failed",
      ),
      AsyncEvent::BookmarksWrite(outcome) => self.on_document_write(
        outcome.result,
        format!("bookmarks updated: {} entries", outcome.changed_bookmarks),
        "bookmarks write failed",
      ),
      AsyncEvent::SearchIndex(outcome) => self.on_search_index(outcome),
      AsyncEvent::SearchPreloadReady { generation } => {
        if self.app.finish_search_preload_delay(generation) {
          self.pump_preload();
        }
        false
      }
      AsyncEvent::SelectionImage(outcome) => self.on_selection_image(outcome),
      AsyncEvent::Overlay(outcome) => {
        let preload = outcome.preload;
        let wanted = self.pipeline.overlays.finish(outcome);
        wanted && !preload
      }
      AsyncEvent::Clipboard(outcome) => {
        self.app.finish_clipboard(outcome);
        true
      }
      AsyncEvent::CacheClear(outcome) => self.on_cache_clear(outcome),
    }
  }

  fn on_input(&mut self, event: crossterm::event::Event, generation: u64) -> bool {
    let current_generation = self.input.generation();
    if generation != current_generation {
      debug!(
        ?event,
        generation, current_generation, "input event ignored because generation is stale"
      );
      return false;
    }
    debug!(?event, generation, "input event accepted");
    let redraw = self.app.handle_input(event, &self.tx);
    self.reset_preloads_if_requested();
    debug!(
      redraw,
      scroll = self.app.scroll,
      focused_page = self.app.focused_page,
      status = %self.app.message,
      "input event handled"
    );
    redraw
  }

  fn on_page(&mut self, outcome: PageOutcome) -> bool {
    if !self.is_current_document(
      outcome.source_size_bytes,
      outcome.source_modified_nanos,
      "page outcome",
    ) {
      return false;
    }
    log_page_outcome(&outcome);
    let preload = outcome.preload;
    // A failed preload is dropped silently; the page is requested again
    // (and its error shown) once it becomes visible.
    let apply = outcome.result.is_ok() || !preload;
    let visible_wait = self.pipeline.pages.finish(&outcome, apply, &self.tx);
    if !apply {
      return visible_wait;
    }
    match outcome.slice {
      Some(slice) => self.app.finish_slice(slice, outcome.result),
      None => self.app.finish_page(outcome.page_index, outcome.result),
    }
    self.pump_preload();
    !preload || visible_wait
  }

  fn on_render(&mut self, outcome: RenderOutcome) -> bool {
    match &outcome.result {
      Ok(rendered) => debug!(
        cache_key = %outcome.cache_key,
        preload = outcome.preload,
        kind = match rendered {
          RenderedImage::Symbols { .. } => "symbols",
          RenderedImage::Protocol { .. } => "protocol",
        },
        "image render completed"
      ),
      Err(error) => warn!(
        cache_key = %outcome.cache_key,
        preload = outcome.preload,
        %error,
        "image render failed"
      ),
    }
    let result = self.pipeline.renderer.finish(outcome, &self.tx);
    if let Some(error) = result.message {
      self.app.set_message(error);
    }
    result.needs_draw
  }

  fn on_document_write(
    &mut self,
    result: Result<DocumentReload, String>,
    success: String,
    failure: &str,
  ) -> bool {
    match result {
      Ok(reload) => {
        self.apply_document_reload(reload);
        info!(message = %success, "document write finished");
        self.app.set_message(success);
      }
      Err(error) => {
        warn!(%error, "{failure}");
        self.app.set_message(format!("{failure}: {error}"));
      }
    }
    true
  }

  fn on_search_index(&mut self, outcome: SearchIndexOutcome) -> bool {
    if !self.is_current_document(
      outcome.source_size_bytes,
      outcome.source_modified_nanos,
      "search index",
    ) {
      return false;
    }
    self.app.finish_search_index(outcome.result);
    self.app.finish_pending_selection_text_copy(&self.tx);
    self.reset_preloads_if_requested();
    self.pump_preload();
    true
  }

  fn on_selection_image(&mut self, outcome: SelectionImageOutcome) -> bool {
    if !self.is_current_document(
      outcome.source_size_bytes,
      outcome.source_modified_nanos,
      "selection image",
    ) {
      return false;
    }
    let redraw = !outcome.preload || self.app.view == ViewMode::Selection;
    self.app.finish_selection_image(outcome);
    self.pump_preload();
    redraw
  }

  fn on_cache_clear(&mut self, outcome: CacheClearOutcome) -> bool {
    match outcome.result {
      Ok(report) => {
        self.app.clear_cached_images();
        self.pipeline.pages.clear_state();
        self.pipeline.overlays.clear();
        self.pipeline.renderer.clear_state();
        self.app.set_message(format!(
          "cache cleared: {} files, {} bytes",
          report.removed_files, report.removed_bytes
        ));
        info!(
          before_bytes = report.before_bytes,
          after_bytes = report.after_bytes,
          removed_files = report.removed_files,
          removed_bytes = report.removed_bytes,
          "cache cleared"
        );
      }
      Err(error) => {
        self.app.set_message(format!("clear-cache failed: {error}"));
        warn!(%error, "cache clear failed");
      }
    }
    true
  }

  /// Results computed for an older version of the file (before a refresh
  /// or an edit) must not be applied to the reloaded document.
  fn is_current_document(&self, size_bytes: u64, modified_nanos: u128, what: &str) -> bool {
    let document = &self.app.document;
    let current = size_bytes == document.size_bytes && modified_nanos == document.modified_nanos;
    if !current {
      debug!(
        source_size_bytes = size_bytes,
        current_size_bytes = document.size_bytes,
        source_modified_nanos = modified_nanos,
        current_modified_nanos = document.modified_nanos,
        "ignored stale {what}"
      );
    }
    current
  }

  fn apply_document_reload(&mut self, reload: DocumentReload) {
    self
      .pipeline
      .pages
      .replace_document(reload.document.clone());
    self.pipeline.overlays.clear();
    self.pipeline.renderer.clear_state();
    self
      .app
      .apply_document_reload(reload.document, reload.metadata, reload.bookmarks);
  }

  fn reset_preloads_if_requested(&mut self) {
    if self.app.take_search_preload_reset() {
      self.pipeline.pages.cancel_preloads();
      self.pipeline.renderer.cancel_preloads();
    }
  }

  fn pump_preload(&mut self) {
    ui::pump_preload(&mut self.app, &mut self.pipeline, &self.tx);
  }
}

fn log_page_outcome(outcome: &PageOutcome) {
  let page = outcome.page_index + 1;
  let preload = outcome.preload;
  match (&outcome.result, outcome.slice) {
    (Ok(image), Some(slice)) => debug!(
      page,
      slice = slice.slice_index + 1,
      slice_count = slice.slice_count,
      preload,
      width = image.width,
      height = image.height,
      path = %image.path.display(),
      metadata = ?image.slice,
      "page slice render completed"
    ),
    (Ok(image), None) => debug!(
      page,
      preload,
      width = image.width,
      height = image.height,
      path = %image.path.display(),
      "page render completed"
    ),
    (Err(error), Some(slice)) => warn!(
      page,
      slice = slice.slice_index + 1,
      slice_count = slice.slice_count,
      preload,
      %error,
      "page slice render failed"
    ),
    (Err(error), None) => warn!(page, preload, %error, "page render failed"),
  }
}
