//! Application state and its transitions. Views, input routing, and
//! navigation live in the submodules; each extends `App`.

mod bookmark_state;
mod commands;
mod input;
mod metadata_state;
mod navigation;
mod progress;
mod search_jump;
mod search_state;
mod selection_geometry;
mod selection_hit;
mod selection_state;
mod tasks;

use std::collections::HashMap;

use framework_tui::{
  CommandCompletion, CommandState, KeyBindings, KeyContext, KeyDispatcher, KeyHelpEntry, KeyHint,
  Prompt,
};
use ratatui::layout::Rect;

use crate::{
  bookmarks::{self, BookmarkEdit, PdfBookmark},
  config::{EffectiveLayoutConfig, Settings},
  event::SelectionImageOutcome,
  metadata::{self, MetadataEdit, PdfMetadataEntry},
  pdf::{PageImage, PageSliceSpec, PdfDocument},
};

use bookmark_state::BookmarkTree;
use search_state::SearchState;
use selection_state::SelectionState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
  Viewer,
  Metadata,
  Bookmarks,
  Search,
  Selection,
}

#[derive(Debug, Clone)]
pub enum EditorRequest {
  Metadata {
    original: Vec<PdfMetadataEntry>,
    draft: String,
  },
  Bookmarks {
    original: Vec<PdfBookmark>,
    draft: String,
  },
}

impl EditorRequest {
  pub fn initial_text(&self) -> &str {
    match self {
      Self::Metadata { draft, .. } => draft,
      Self::Bookmarks { draft, .. } => draft,
    }
  }
}

#[derive(Debug, Clone)]
pub enum ConfirmDialog {
  MetadataWrite { edit: MetadataEdit },
  BookmarksWrite { edit: BookmarkEdit },
}

pub struct App {
  pub document: PdfDocument,
  pub settings: Settings,
  keymap: KeyBindings,
  bookmarks_keymap: KeyBindings,
  search_keymap: KeyBindings,
  selection_keymap: KeyBindings,
  pages: Vec<Option<PageImage>>,
  slices: HashMap<PageSliceSpec, PageImage>,
  page_errors: Vec<Option<String>>,
  slice_errors: HashMap<PageSliceSpec, String>,
  pub layout: EffectiveLayoutConfig,
  pub scroll: u32,
  pub grid_start_page: usize,
  pub focused_page: usize,
  pub viewport: Option<Rect>,
  viewport_height: u16,
  scroll_layout: Option<navigation::CachedScrollLayout>,
  pub terminal_cell_pixels: Option<(u16, u16)>,
  pub prompt: Option<Prompt>,
  pub view: ViewMode,
  pub metadata: Vec<PdfMetadataEntry>,
  pub metadata_error: Option<String>,
  pub metadata_scroll: u16,
  pub bookmarks: BookmarkTree,
  pub search: SearchState,
  pub selection: SelectionState,
  pub confirm: Option<ConfirmDialog>,
  pub key_help: bool,
  pub message: String,
  frame_navigation_locked: bool,
  pending_progress: Option<f64>,
  refresh_in_flight: bool,
  refresh_queued: bool,
  quit: bool,
  editor_request: Option<EditorRequest>,
  command_state: CommandState,
  key_dispatcher: KeyDispatcher,
}

impl App {
  pub fn new(document: PdfDocument, settings: Settings) -> Self {
    let keymap = settings.keymap.bindings();
    let bookmarks_keymap = settings.keymap.bookmarks_bindings();
    let search_keymap = settings.keymap.search_bindings();
    let selection_keymap = settings.keymap.selection_bindings();
    let layout = settings.config.layout.effective();
    let page_count = document.page_count;
    let (metadata, outline) =
      tasks::read_document_info(&document, &settings.config.render.pdftk_bin);
    let (metadata, metadata_error) = match metadata {
      Ok(metadata) => (metadata, None),
      Err(error) => (Vec::new(), Some(error)),
    };
    let bookmarks = BookmarkTree::new(
      outline,
      settings.config.behavior.bookmarks_left_ratio,
      settings.config.behavior.bookmarks_right_ratio,
    );
    let search = SearchState::new(
      settings.config.behavior.search_left_ratio,
      settings.config.behavior.search_right_ratio,
    );
    Self {
      document,
      settings,
      keymap,
      bookmarks_keymap,
      search_keymap,
      selection_keymap,
      pages: vec![None; page_count],
      slices: HashMap::new(),
      page_errors: vec![None; page_count],
      slice_errors: HashMap::new(),
      layout,
      scroll: 0,
      grid_start_page: 0,
      focused_page: 0,
      viewport: None,
      viewport_height: 1,
      scroll_layout: None,
      terminal_cell_pixels: None,
      prompt: None,
      view: ViewMode::Viewer,
      metadata,
      metadata_error,
      metadata_scroll: 0,
      bookmarks,
      search,
      selection: SelectionState::default(),
      confirm: None,
      key_help: false,
      message: "ready".to_string(),
      frame_navigation_locked: false,
      pending_progress: None,
      refresh_in_flight: false,
      refresh_queued: false,
      quit: false,
      editor_request: None,
      command_state: CommandState::default(),
      key_dispatcher: KeyDispatcher::default(),
    }
  }

  pub fn should_quit(&self) -> bool {
    self.quit
  }

  pub fn key_hints(&self) -> &[KeyHint] {
    self.key_dispatcher.hints()
  }

  pub fn command_completion(&self) -> Option<&CommandCompletion> {
    self.command_state.completion()
  }

  pub fn take_editor_request(&mut self) -> Option<EditorRequest> {
    self.editor_request.take()
  }

  pub fn set_message(&mut self, message: impl Into<String>) {
    self.message = message.into();
  }

  pub fn set_editor_request(&mut self, request: EditorRequest) {
    self.editor_request = Some(request);
  }

  pub fn finish_frame_render_pass(&mut self, fully_rendered: bool) {
    if fully_rendered {
      self.frame_navigation_locked = false;
    }
  }

  pub(super) fn lock_frame_navigation_if_enabled(&mut self) {
    if self.frame_sync_navigation_enabled() {
      self.frame_navigation_locked = true;
    }
  }

  pub(super) fn frame_sync_navigation_enabled(&self) -> bool {
    let behavior = &self.settings.config.behavior;
    match self.view {
      ViewMode::Viewer => behavior.frame_sync_navigation_viewer,
      ViewMode::Bookmarks => behavior.frame_sync_navigation_bookmarks,
      ViewMode::Search => behavior.frame_sync_navigation_search,
      ViewMode::Selection => false,
      ViewMode::Metadata => false,
    }
  }

  pub(super) fn clear_frame_navigation_lock(&mut self) {
    self.frame_navigation_locked = false;
  }

  pub fn clear_cached_images(&mut self) {
    self.pages.fill(None);
    self.slices.clear();
    self.page_errors.fill(None);
    self.slice_errors.clear();
    self.selection.clear_images();
    self.lock_frame_navigation_if_enabled();
  }

  pub fn apply_document_reload(
    &mut self,
    document: PdfDocument,
    metadata: Result<Vec<PdfMetadataEntry>, String>,
    bookmarks: Result<Vec<PdfBookmark>, String>,
  ) {
    let progress = self.current_progress().or(self.pending_progress);
    let page_count = document.page_count;
    self.document = document;
    self.pages = vec![None; page_count];
    self.slices.clear();
    self.page_errors = vec![None; page_count];
    self.slice_errors.clear();
    self.scroll_layout = None;
    self.focused_page = self.focused_page.min(page_count.saturating_sub(1));
    self.grid_start_page = self.grid_start_page.min(page_count.saturating_sub(1));
    self.scroll = 0;
    match metadata {
      Ok(metadata) => {
        self.metadata = metadata;
        self.metadata_error = None;
      }
      Err(error) => {
        self.metadata.clear();
        self.metadata_error = Some(error);
      }
    }
    self.bookmarks.replace(bookmarks);
    self.search.reset_for_reload();
    self.selection = SelectionState::default();
    self.metadata_scroll = 0;
    if let Some(progress) = progress {
      self.set_progress_target(progress);
    } else {
      self.normalize_current_layout_state();
    }
    self.lock_frame_navigation_if_enabled();
  }

  pub fn finish_metadata_editor_input(
    &mut self,
    original: Vec<PdfMetadataEntry>,
    result: Result<String, String>,
  ) {
    let edited = match result {
      Ok(edited) => edited,
      Err(error) => {
        self.set_message(format!("editor failed: {error}"));
        return;
      }
    };
    let edit = match metadata::metadata_changes_from_edit(&original, &edited) {
      Ok(edit) => edit,
      Err(error) => {
        self.set_message(format!("metadata edit failed: {error}"));
        return;
      }
    };
    if edit.is_empty() {
      self.set_message("metadata unchanged");
      return;
    }
    let count = edit.change_count();
    self.confirm = Some(ConfirmDialog::MetadataWrite { edit });
    self.set_message(format!("confirm metadata changes: {count} change(s)"));
  }

  pub fn finish_bookmarks_editor_input(
    &mut self,
    original: Vec<PdfBookmark>,
    result: Result<String, String>,
  ) {
    let edited = match result {
      Ok(edited) => edited,
      Err(error) => {
        self.set_message(format!("editor failed: {error}"));
        return;
      }
    };
    let edit =
      match bookmarks::bookmark_changes_from_edit(&original, &edited, self.document.page_count) {
        Ok(Some(edit)) => edit,
        Ok(None) => {
          self.set_message("bookmarks unchanged");
          return;
        }
        Err(error) => {
          self.set_message(format!("bookmark edit failed: {error}"));
          return;
        }
      };
    self.set_message(format!(
      "confirm bookmark changes: {} -> {} entries",
      edit.original_count,
      edit.new_count()
    ));
    self.confirm = Some(ConfirmDialog::BookmarksWrite { edit });
  }

  pub fn finish_refresh_request(&mut self) -> bool {
    self.refresh_in_flight = false;
    std::mem::take(&mut self.refresh_queued)
  }

  pub fn show_key_help(&mut self) {
    self.key_help = true;
    self.key_dispatcher.clear();
    self.set_message("key bindings");
  }

  pub fn key_help_title(&self) -> &'static str {
    if self.prompt.is_some() {
      return "Input key bindings";
    }
    match self.view {
      ViewMode::Viewer => "Viewer key bindings",
      ViewMode::Metadata => "Metadata key bindings",
      ViewMode::Bookmarks => "Bookmark key bindings",
      ViewMode::Search => "Search key bindings",
      ViewMode::Selection => "Selection key bindings",
    }
  }

  pub fn key_help_entries(&self) -> Vec<KeyHelpEntry> {
    if self.prompt.is_some() {
      return self
        .keymap
        .help_entries_filtered(KeyContext::Input, |action| {
          input_action_available(action, true)
        });
    }

    if self.view == ViewMode::Bookmarks {
      return self
        .bookmarks_keymap
        .help_entries_filtered(KeyContext::Browser, |action| self.action_available(action));
    }

    if self.view == ViewMode::Search {
      return self
        .search_keymap
        .help_entries_filtered(KeyContext::Browser, |action| self.action_available(action));
    }

    if self.view == ViewMode::Selection {
      return self
        .selection_keymap
        .help_entries_filtered(KeyContext::Browser, |action| self.action_available(action));
    }

    self
      .keymap
      .help_entries_filtered(self.key_context(), |action| self.action_available(action))
  }

  pub fn key_context(&self) -> KeyContext {
    match self.view {
      ViewMode::Viewer => KeyContext::Browser,
      ViewMode::Metadata => KeyContext::Detail,
      ViewMode::Bookmarks => KeyContext::Browser,
      ViewMode::Search => KeyContext::Browser,
      ViewMode::Selection => KeyContext::Browser,
    }
  }

  pub fn action_available(&self, action: &str) -> bool {
    if action.starts_with("layout ") || action.starts_with("layout-use ") {
      return self.view == ViewMode::Viewer;
    }
    match action {
      "quit" | "command" | "help" => true,
      "clear-cache" | "clear_cache" | "refresh" => true,
      "back" => matches!(
        self.view,
        ViewMode::Metadata | ViewMode::Bookmarks | ViewMode::Search | ViewMode::Selection
      ),
      "scroll_down" | "scroll_up" | "page_down" | "page_up" | "next_page" | "previous_page"
      | "home" | "end" | "metadata" | "bookmarks" | "search" | "selection" => {
        self.view == ViewMode::Viewer
      }
      "selection_mark" | "selection_cancel" => {
        matches!(self.view, ViewMode::Viewer | ViewMode::Selection)
      }
      "edit_metadata"
      | "metadata_scroll_down"
      | "metadata_scroll_up"
      | "metadata_page_down"
      | "metadata_page_up" => self.view == ViewMode::Metadata,
      "edit_bookmarks"
      | "bookmarks_next"
      | "bookmarks_previous"
      | "bookmarks_page_down"
      | "bookmarks_page_up"
      | "bookmarks_toggle"
      | "bookmarks_toggle_all"
      | "bookmarks_open"
      | "bookmarks_panel_narrower"
      | "bookmarks_panel_wider" => self.view == ViewMode::Bookmarks,
      "search_next" | "search_previous" | "search_page_down" | "search_page_up" | "search_open" => {
        self.view == ViewMode::Search
      }
      "selection_next"
      | "selection_previous"
      | "selection_reselect"
      | "selection_copy_text"
      | "selection_copy_image" => self.view == ViewMode::Selection,
      _ => false,
    }
  }

  pub fn finish_page(&mut self, page_index: usize, result: Result<PageImage, String>) {
    if page_index >= self.pages.len() {
      return;
    }
    match result {
      Ok(page) => {
        self.pages[page_index] = Some(page);
        self.page_errors[page_index] = None;
      }
      Err(error) => {
        self.page_errors[page_index] = Some(error.clone());
        self.set_message(format!("page {} failed: {error}", page_index + 1));
      }
    }
  }

  pub fn finish_slice(&mut self, spec: PageSliceSpec, result: Result<PageImage, String>) {
    match result {
      Ok(slice) => {
        self.slices.insert(spec, slice);
        self.slice_errors.remove(&spec);
      }
      Err(error) => {
        self.slice_errors.insert(spec, error.clone());
        self.set_message(format!(
          "page {} slice {}/{} failed: {error}",
          spec.page_index + 1,
          spec.slice_index + 1,
          spec.slice_count
        ));
      }
    }
  }

  pub fn finish_selection_image(&mut self, outcome: SelectionImageOutcome) {
    self.selection.image_in_flight.remove(&outcome.key);
    match outcome.result {
      Ok(image) => {
        self.selection.image_errors.remove(&outcome.key);
        self.selection.images.insert(outcome.key, image);
      }
      Err(error) => {
        self.selection.images.remove(&outcome.key);
        self
          .selection
          .image_errors
          .insert(outcome.key.clone(), error.clone());
        if !outcome.preload {
          self.set_message(format!("selection image failed: {error}"));
        }
      }
    }
  }

  pub fn page_image(&self, index: usize) -> Option<&PageImage> {
    self.pages.get(index)?.as_ref()
  }

  pub fn page_error(&self, index: usize) -> Option<&str> {
    self.page_errors.get(index)?.as_deref()
  }

  pub fn slice_image(&self, spec: &PageSliceSpec) -> Option<&PageImage> {
    self.slices.get(spec)
  }

  pub fn slice_error(&self, spec: &PageSliceSpec) -> Option<&str> {
    self.slice_errors.get(spec).map(String::as_str)
  }

  pub fn page_dimensions(&self, index: usize) -> Option<(u32, u32)> {
    if index >= self.document.page_count {
      return None;
    }
    Some(self.document.logical_page_size(index))
  }

  pub fn update_viewport(&mut self, viewport: Rect) {
    self.viewport = Some(viewport);
    self.viewport_height = viewport.height.max(1);
  }

  pub fn set_grid_viewport(&mut self, viewport: Rect, capacity: usize) {
    self.update_viewport(viewport);
    self.scroll_layout = None;
    self.clamp_grid_start(capacity);
    self.apply_pending_progress_if_ready();
    self.focused_page = self
      .grid_start_page
      .min(self.document.page_count.saturating_sub(1));
  }
}

fn input_action_available(action: &str, command_prompt: bool) -> bool {
  command_prompt
    || !matches!(
      action,
      "completion_next" | "completion_previous" | "history_previous" | "history_next"
    )
}
