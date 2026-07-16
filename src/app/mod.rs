mod input;
mod navigation;
mod progress;

use framework_tui::{CommandCompletion, CommandState, KeyBindings, KeyDispatcher, KeyHint, Prompt};
use ratatui::layout::Rect;

use crate::{
  config::{EffectiveLayoutConfig, Settings},
  layout::ScrollLayout,
  pdf::{PageImage, PageSliceSpec, PdfDocument},
};

pub struct App {
  pub document: PdfDocument,
  pub settings: Settings,
  pub keymap: KeyBindings,
  pub pages: Vec<Option<PageImage>>,
  pub slices: std::collections::HashMap<PageSliceSpec, PageImage>,
  pub page_errors: Vec<Option<String>>,
  pub slice_errors: std::collections::HashMap<PageSliceSpec, String>,
  pub layout: EffectiveLayoutConfig,
  pub scroll: u32,
  pub grid_start_page: usize,
  pub focused_page: usize,
  pub viewport: Option<Rect>,
  pub viewport_height: u16,
  pub last_scroll_layout: Option<ScrollLayout>,
  pub terminal_cell_pixels: Option<(u16, u16)>,
  pub prompt: Option<Prompt>,
  pub message: String,
  pending_progress: Option<f64>,
  quit: bool,
  command_state: CommandState,
  key_dispatcher: KeyDispatcher,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InputRedrawState {
  scroll: u32,
  grid_start_page: usize,
  focused_page: usize,
  layout: String,
  message: String,
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
    let layout = settings.config.layout.effective();
    let page_count = document.page_count;
    Self {
      document,
      settings,
      keymap,
      pages: vec![None; page_count],
      slices: std::collections::HashMap::new(),
      page_errors: vec![None; page_count],
      slice_errors: std::collections::HashMap::new(),
      layout,
      scroll: 0,
      grid_start_page: 0,
      focused_page: 0,
      viewport: None,
      viewport_height: 1,
      last_scroll_layout: None,
      terminal_cell_pixels: None,
      prompt: None,
      message: "ready".to_string(),
      pending_progress: None,
      quit: false,
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

  pub fn set_message(&mut self, message: impl Into<String>) {
    self.message = message.into();
  }

  pub fn clear_cached_images(&mut self) {
    self.pages.fill(None);
    self.slices.clear();
    self.page_errors.fill(None);
    self.slice_errors.clear();
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

  pub fn page_dimensions(&self, index: usize) -> Option<(u32, u32)> {
    if index >= self.document.page_count {
      return None;
    }
    Some(self.document.logical_page_size())
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
