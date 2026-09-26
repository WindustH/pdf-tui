//! Background jobs started from the UI: document reloads, PDF edits,
//! cache clearing, the search index, selection crops, and clipboard copies.
//! Each reports back through an `AsyncEvent`.

use std::path::PathBuf;

use tokio::sync::mpsc;

use crate::{
  bookmarks::{self, PdfBookmark},
  cache, clipboard, config,
  event::{
    AsyncEvent, BookmarksWriteOutcome, CacheClearOutcome, ClipboardKind, ClipboardOutcome,
    DocumentReload, DocumentReloadOutcome, MetadataWriteOutcome, SearchIndexOutcome,
    SelectionImageOutcome,
  },
  metadata::{self, PdfMetadataEntry},
  pdf::PdfDocument,
  search,
  selection::{self, PdfSelection},
};

use super::{App, ConfirmDialog};

impl App {
  pub(super) fn apply_confirm(&mut self, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    let Some(confirm) = self.confirm.take() else {
      return;
    };
    match confirm {
      ConfirmDialog::MetadataWrite { edit } => {
        let path = self.document.path.clone();
        let cache_dir = self.document.page_cache_dir.clone();
        let render = self.settings.config.render.clone();
        let changed_tags = edit.tags.len();
        let tx = tx.clone();
        self.set_message(format!("applying metadata edit: {changed_tags} tag(s)"));
        tokio::task::spawn_blocking(move || {
          let result = (|| {
            metadata::write_pdf_metadata_with_exiftool(&path, &edit.tags)?;
            reload_document(path, cache_dir, render)
          })();
          let _ = tx.send(AsyncEvent::MetadataWrite(MetadataWriteOutcome {
            result,
            changed_tags,
          }));
        });
      }
      ConfirmDialog::BookmarksWrite { edit } => {
        let path = self.document.path.clone();
        let cache_dir = self.document.page_cache_dir.clone();
        let app_cache_dir = self.settings.cache_dir.clone();
        let render = self.settings.config.render.clone();
        let pdftk_bin = self.settings.config.render.pdftk_bin.clone();
        let changed_bookmarks = edit.new_count();
        let tx = tx.clone();
        self.set_message(format!(
          "applying bookmark edit: {changed_bookmarks} entries"
        ));
        tokio::task::spawn_blocking(move || {
          let result = (|| {
            bookmarks::write_pdf_bookmarks_with_pdftk(
              &path,
              &pdftk_bin,
              &app_cache_dir,
              &edit.bookmarks,
            )?;
            reload_document(path, cache_dir, render)
          })();
          let _ = tx.send(AsyncEvent::BookmarksWrite(BookmarksWriteOutcome {
            result,
            changed_bookmarks,
          }));
        });
      }
    }
  }

  pub(super) fn request_clear_cache(&mut self, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    let cache_dir = self.settings.cache_dir.clone();
    let tx = tx.clone();
    self.set_message("clearing cache...");
    tokio::spawn(async move {
      let result = cache::clear_cache(&cache_dir)
        .await
        .map_err(|error| error.to_string());
      let _ = tx.send(AsyncEvent::CacheClear(CacheClearOutcome { result }));
    });
  }

  pub fn request_refresh(&mut self, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    if self.refresh_in_flight {
      self.refresh_queued = true;
      self.set_message("refresh already running; queued one more refresh");
      return;
    }
    let path = self.document.path.clone();
    let cache_dir = self.document.page_cache_dir.clone();
    let render = self.settings.config.render.clone();
    let tx = tx.clone();
    self.refresh_in_flight = true;
    self.set_message("refreshing document...");
    tokio::task::spawn_blocking(move || {
      let result = reload_document(path, cache_dir, render);
      let _ = tx.send(AsyncEvent::Refresh(DocumentReloadOutcome { result }));
    });
  }

  pub fn request_search_index(&mut self, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    if self.search.index.is_some() || self.search.index_loading {
      return;
    }
    self.search.index_loading = true;
    self.search.index_error = None;
    let path = self.document.path.clone();
    let cache_dir = self.settings.cache_dir.clone();
    let pdftotext_bin = self.settings.config.render.pdftotext_bin.clone();
    let page_count = self.document.page_count;
    let source_size_bytes = self.document.size_bytes;
    let source_modified_nanos = self.document.modified_nanos;
    let tx = tx.clone();
    self.set_message("building search index...");
    tokio::spawn(async move {
      let result = search::build_search_index(
        &path,
        &cache_dir,
        &pdftotext_bin,
        page_count,
        source_size_bytes,
        source_modified_nanos,
      )
      .await;
      let _ = tx.send(AsyncEvent::SearchIndex(SearchIndexOutcome {
        source_size_bytes,
        source_modified_nanos,
        result,
      }));
    });
  }

  pub(super) fn selection_copy_text(&mut self, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    if self.selection.copy_text_pending {
      return;
    }
    if self.current_selection().is_none() {
      self.set_message("no selection");
      return;
    }
    if self.search.index.is_none() {
      self.selection.copy_text_pending = true;
      self.request_search_index(tx);
      self.set_message("building text index for selection...");
      return;
    }
    self.copy_selection_text_from_ready_index(tx);
  }

  pub fn finish_pending_selection_text_copy(&mut self, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    if !self.selection.copy_text_pending {
      return;
    }
    self.selection.copy_text_pending = false;
    self.copy_selection_text_from_ready_index(tx);
  }

  pub(super) fn selection_copy_image(&mut self, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    if self.selection.copy_image_pending {
      return;
    }
    let Some(selection) = self.current_selection().copied() else {
      self.set_message("no selection");
      return;
    };
    let document = self.document.clone();
    let cache_dir = self.settings.cache_dir.clone();
    let max_pixels = self.settings.config.render.selection_image_max_pixels;
    let cache_max_bytes = self.settings.config.render.selection_cache_max_bytes;
    let tx = tx.clone();
    self.selection.copy_image_pending = true;
    self.set_message("copying selected image...");
    tokio::spawn(async move {
      let result = selection::render_selection_copy_image(
        document,
        selection,
        cache_dir,
        max_pixels,
        cache_max_bytes,
      )
      .await;
      let result = match result {
        Ok(path) => match clipboard::copy_png(&path).await {
          Ok(()) => Ok(format!("copied selected image: {}", path.display())),
          Err(error) => Err(error),
        },
        Err(error) => Err(error),
      };
      let _ = tx.send(AsyncEvent::Clipboard(ClipboardOutcome {
        kind: ClipboardKind::SelectionImage,
        result,
      }));
    });
  }

  pub fn request_selection_image(
    &mut self,
    selection: PdfSelection,
    target_width: u32,
    target_height: u32,
    preload: bool,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) -> String {
    let target_width = target_width.max(1);
    let target_height = target_height.max(1);
    let key = selection::selection_image_cache_key(
      &self.document,
      selection,
      target_width,
      target_height,
      "preview",
    );
    if self.selection.images.contains_key(&key)
      || self.selection.image_errors.contains_key(&key)
      || self.selection.image_in_flight.contains(&key)
    {
      return key;
    }
    if selection.page_index >= self.document.page_count {
      self.selection.image_errors.insert(
        key.clone(),
        format!("page {} is outside the document", selection.page_index + 1),
      );
      return key;
    }
    self.selection.image_in_flight.insert(key.clone());
    let document = self.document.clone();
    let source_size_bytes = document.size_bytes;
    let source_modified_nanos = document.modified_nanos;
    let cache_dir = self.settings.cache_dir.clone();
    let cache_max_bytes = self.settings.config.render.selection_cache_max_bytes;
    let key_for_task = key.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
      let result = selection::render_selection_preview_image(
        document,
        selection,
        cache_dir,
        target_width,
        target_height,
        cache_max_bytes,
      )
      .await;
      let _ = tx.send(AsyncEvent::SelectionImage(SelectionImageOutcome {
        source_size_bytes,
        source_modified_nanos,
        key: key_for_task,
        preload,
        result,
      }));
    });
    key
  }

  pub fn finish_clipboard(&mut self, outcome: ClipboardOutcome) {
    match outcome.kind {
      ClipboardKind::SelectionText => self.selection.copy_text_pending = false,
      ClipboardKind::SelectionImage => self.selection.copy_image_pending = false,
    }
    match outcome.result {
      Ok(message) => self.set_message(message),
      Err(error) => self.set_message(error),
    }
  }

  fn copy_selection_text_from_ready_index(&mut self, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    let Some(selection) = self.current_selection().copied() else {
      self.set_message("no selection");
      return;
    };
    let Some(index) = &self.search.index else {
      let message = self
        .search
        .index_error
        .as_ref()
        .map(|error| format!("selection text unavailable: {error}"))
        .unwrap_or_else(|| "selection text index is not ready".to_string());
      self.set_message(message);
      return;
    };
    let text = index.text_in_selection(selection);
    let chars = text.chars().count();
    let tx = tx.clone();
    self.selection.copy_text_pending = true;
    self.set_message("copying selected text...");
    tokio::spawn(async move {
      let result = clipboard::copy_text(text)
        .await
        .map(|()| format!("copied selected text: {chars} chars"));
      let _ = tx.send(AsyncEvent::Clipboard(ClipboardOutcome {
        kind: ClipboardKind::SelectionText,
        result,
      }));
    });
  }
}

fn reload_document(
  path: PathBuf,
  cache_dir: PathBuf,
  render: config::RenderConfig,
) -> Result<DocumentReload, String> {
  let document =
    PdfDocument::open(path.clone(), cache_dir, &render).map_err(|error| error.to_string())?;
  let (metadata, bookmarks) = read_document_info(&document, &render.pdftk_bin);
  Ok(DocumentReload {
    document,
    metadata,
    bookmarks,
  })
}

/// Metadata (`exiftool`) and outline (`pdftk`) of `document`. The two
/// tools run concurrently: each takes from tens of milliseconds to a second
/// (pdftk-java starts a JVM), and startup waits for both.
pub(super) fn read_document_info(
  document: &PdfDocument,
  pdftk_bin: &str,
) -> (
  Result<Vec<PdfMetadataEntry>, String>,
  Result<Vec<PdfBookmark>, String>,
) {
  std::thread::scope(|scope| {
    let metadata = scope.spawn(|| metadata::read_pdf_metadata(&document.path));
    let bookmarks = bookmarks::read_pdf_bookmarks(&document.path, pdftk_bin, document.page_count);
    let metadata = metadata
      .join()
      .unwrap_or_else(|_| Err("metadata reader panicked".to_string()));
    (metadata, bookmarks)
  })
}
