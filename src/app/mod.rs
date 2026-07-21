mod bookmark_state;
mod input;
mod navigation;
mod progress;
mod search_state;
mod selection_state;

use std::collections::{HashMap, HashSet};

use framework_tui::{
  CommandCompletion, CommandState, KeyBindings, KeyContext, KeyDispatcher, KeyHelpEntry, KeyHint,
  Prompt,
};
use ratatui::layout::Rect;
use tokio::{sync::mpsc, time::sleep};

use crate::{
  bookmarks::{self, BookmarkEdit, PdfBookmark},
  config::{EffectiveLayoutConfig, Settings},
  event::{AsyncEvent, SelectionImageOutcome},
  layout::ScrollLayout,
  metadata::{self, MetadataEdit, PdfMetadataEntry},
  pdf::{PageImage, PageSliceSpec, PdfDocument},
  search::{PdfSearchIndex, PdfSearchMatch},
  selection::{PdfRect, PdfSelection, SelectionAnchor},
};

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

#[derive(Debug, Clone, PartialEq)]
pub struct SelectionDisplay {
  pub selection_index: usize,
  pub page_index: usize,
  pub page_width: f64,
  pub page_height: f64,
  pub rect: PdfRect,
  pub area: Rect,
}

pub struct App {
  pub document: PdfDocument,
  pub settings: Settings,
  pub keymap: KeyBindings,
  pub bookmarks_keymap: KeyBindings,
  pub search_keymap: KeyBindings,
  pub selection_keymap: KeyBindings,
  pub pages: Vec<Option<PageImage>>,
  pub slices: HashMap<PageSliceSpec, PageImage>,
  pub page_errors: Vec<Option<String>>,
  pub slice_errors: HashMap<PageSliceSpec, String>,
  pub layout: EffectiveLayoutConfig,
  pub scroll: u32,
  pub grid_start_page: usize,
  pub focused_page: usize,
  pub viewport: Option<Rect>,
  pub viewport_height: u16,
  pub last_scroll_layout: Option<ScrollLayout>,
  pub terminal_cell_pixels: Option<(u16, u16)>,
  pub prompt: Option<Prompt>,
  pub view: ViewMode,
  pub metadata: Vec<PdfMetadataEntry>,
  pub metadata_error: Option<String>,
  pub metadata_scroll: u16,
  pub bookmarks: Vec<PdfBookmark>,
  pub bookmarks_error: Option<String>,
  pub bookmarks_expanded: HashSet<usize>,
  pub bookmarks_selected: Option<usize>,
  pub bookmarks_scroll: u16,
  pub bookmarks_all_expanded: bool,
  pub bookmarks_left_ratio: u16,
  pub bookmarks_right_ratio: u16,
  pub search_prompt: Prompt,
  pub search_index: Option<PdfSearchIndex>,
  pub search_index_error: Option<String>,
  pub search_index_loading: bool,
  pub search_results: Vec<PdfSearchMatch>,
  pub search_selected: Option<usize>,
  pub search_scroll: u16,
  pub search_left_ratio: u16,
  pub search_right_ratio: u16,
  search_preload_generation: u64,
  search_preload_ready_generation: u64,
  search_preload_reset_pending: bool,
  pub selection_anchor: Option<SelectionAnchor>,
  pub selection_second_anchor: Option<SelectionAnchor>,
  selection_draft_index: Option<usize>,
  pub selection_display: Option<SelectionDisplay>,
  pub selection_images: HashMap<String, PageImage>,
  pub selection_image_errors: HashMap<String, String>,
  selection_image_in_flight: HashSet<String>,
  pub selections: Vec<PdfSelection>,
  pub selection_index: Option<usize>,
  selection_copy_text_pending: bool,
  selection_copy_image_pending: bool,
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
  search_command_state: CommandState,
  key_dispatcher: KeyDispatcher,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InputRedrawState {
  scroll: u32,
  grid_start_page: usize,
  focused_page: usize,
  view: ViewMode,
  metadata_scroll: u16,
  bookmarks_selected: Option<usize>,
  bookmarks_scroll: u16,
  bookmarks_expanded_len: usize,
  bookmarks_left_ratio: u16,
  bookmarks_right_ratio: u16,
  search_input: String,
  search_cursor: usize,
  search_results_len: usize,
  search_selected: Option<usize>,
  search_scroll: u16,
  search_index_loading: bool,
  search_index_error: Option<String>,
  selection_anchor_active: bool,
  selection_anchor_state: Option<String>,
  selections_len: usize,
  selection_index: Option<usize>,
  selection_copy_text_pending: bool,
  selection_copy_image_pending: bool,
  confirm: bool,
  key_help: bool,
  editor_request: bool,
  layout: String,
  message: String,
  frame_navigation_locked: bool,
  quit: bool,
  prompt: Option<PromptRedrawState>,
  completion: Option<CompletionRedrawState>,
  key_hint_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PromptRedrawState {
  prefix: String,
  input: String,
  cursor: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompletionRedrawState {
  candidates: Vec<String>,
  selected: usize,
}

impl App {
  pub fn new(document: PdfDocument, settings: Settings) -> Self {
    let keymap = settings.keymap.bindings();
    let bookmarks_keymap = settings.keymap.bookmarks_bindings();
    let search_keymap = settings.keymap.search_bindings();
    let selection_keymap = settings.keymap.selection_bindings();
    let layout = settings.config.layout.effective();
    let page_count = document.page_count;
    let (metadata, metadata_error) = match metadata::read_pdf_metadata(&document.path) {
      Ok(metadata) => (metadata, None),
      Err(error) => (Vec::new(), Some(error)),
    };
    let (bookmarks, bookmarks_error) = match bookmarks::read_pdf_bookmarks(
      &document.path,
      &settings.config.render.pdftk_bin,
      document.page_count,
    ) {
      Ok(bookmarks) => (bookmarks, None),
      Err(error) => (Vec::new(), Some(error)),
    };
    let bookmarks_left_ratio = settings.config.behavior.bookmarks_left_ratio.max(1);
    let bookmarks_right_ratio = settings.config.behavior.bookmarks_right_ratio.max(1);
    let search_left_ratio = settings.config.behavior.search_left_ratio.max(1);
    let search_right_ratio = settings.config.behavior.search_right_ratio.max(1);
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
      last_scroll_layout: None,
      terminal_cell_pixels: None,
      prompt: None,
      view: ViewMode::Viewer,
      metadata,
      metadata_error,
      metadata_scroll: 0,
      bookmarks,
      bookmarks_error,
      bookmarks_expanded: HashSet::new(),
      bookmarks_selected: None,
      bookmarks_scroll: 0,
      bookmarks_all_expanded: false,
      bookmarks_left_ratio,
      bookmarks_right_ratio,
      search_prompt: Prompt::text("search: ", ""),
      search_index: None,
      search_index_error: None,
      search_index_loading: false,
      search_results: Vec::new(),
      search_selected: None,
      search_scroll: 0,
      search_left_ratio,
      search_right_ratio,
      search_preload_generation: 0,
      search_preload_ready_generation: 0,
      search_preload_reset_pending: false,
      selection_anchor: None,
      selection_second_anchor: None,
      selection_draft_index: None,
      selection_display: None,
      selection_images: HashMap::new(),
      selection_image_errors: HashMap::new(),
      selection_image_in_flight: HashSet::new(),
      selections: Vec::new(),
      selection_index: None,
      selection_copy_text_pending: false,
      selection_copy_image_pending: false,
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
      search_command_state: CommandState::default(),
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

  pub fn search_preload_ready(&self) -> bool {
    self.view == ViewMode::Search
      && self.search_preload_ready_generation == self.search_preload_generation
  }

  pub fn finish_search_preload_delay(&mut self, generation: u64) -> bool {
    if self.view != ViewMode::Search || generation != self.search_preload_generation {
      return false;
    }
    self.search_preload_ready_generation = generation;
    true
  }

  pub fn take_search_preload_reset(&mut self) -> bool {
    std::mem::take(&mut self.search_preload_reset_pending)
  }

  pub(super) fn make_search_preload_ready_now(&mut self) {
    self.search_preload_ready_generation = self.search_preload_generation;
  }

  pub(super) fn defer_search_preload_after_input(
    &mut self,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) {
    self.search_preload_generation = self.search_preload_generation.wrapping_add(1);
    self.search_preload_reset_pending = true;
    let generation = self.search_preload_generation;
    let delay =
      std::time::Duration::from_millis(self.settings.config.render.search_preload_idle_ms);
    let tx = tx.clone();
    tokio::spawn(async move {
      sleep(delay).await;
      let _ = tx.send(AsyncEvent::SearchPreloadReady { generation });
    });
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
    self.selection_images.clear();
    self.selection_image_errors.clear();
    self.selection_image_in_flight.clear();
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
    self.last_scroll_layout = None;
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
    match bookmarks {
      Ok(bookmarks) => {
        self.bookmarks = bookmarks;
        self.bookmarks_error = None;
      }
      Err(error) => {
        self.bookmarks.clear();
        self.bookmarks_error = Some(error);
      }
    }
    self.bookmarks_expanded.clear();
    self.bookmarks_selected = None;
    self.bookmarks_scroll = 0;
    self.bookmarks_all_expanded = false;
    self.search_index = None;
    self.search_index_error = None;
    self.search_index_loading = false;
    self.search_results.clear();
    self.search_selected = None;
    self.search_scroll = 0;
    self.selection_anchor = None;
    self.selection_second_anchor = None;
    self.selection_draft_index = None;
    self.selection_display = None;
    self.selection_images.clear();
    self.selection_image_errors.clear();
    self.selection_image_in_flight.clear();
    self.selections.clear();
    self.selection_index = None;
    self.selection_copy_text_pending = false;
    self.selection_copy_image_pending = false;
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
    self.selection_image_in_flight.remove(&outcome.key);
    match outcome.result {
      Ok(image) => {
        self.selection_image_errors.remove(&outcome.key);
        self.selection_images.insert(outcome.key, image);
      }
      Err(error) => {
        self.selection_images.remove(&outcome.key);
        self
          .selection_image_errors
          .insert(outcome.key.clone(), error.clone());
        if !outcome.preload {
          self.set_message(format!("selection image failed: {error}"));
        }
      }
    }
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

  pub fn update_scroll_layout(&mut self, layout: ScrollLayout, viewport: Rect) {
    self.update_viewport(viewport);
    let max_scroll = crate::layout::max_scroll_row_for_viewport(
      &layout,
      viewport.height,
      self.layout.scroll_divisor,
    ) as u32;
    self.scroll = self.scroll.min(max_scroll);
    self.last_scroll_layout = Some(layout);
    self.apply_pending_progress_if_ready();
  }

  pub fn set_grid_viewport(&mut self, viewport: Rect, capacity: usize) {
    self.update_viewport(viewport);
    self.last_scroll_layout = None;
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
