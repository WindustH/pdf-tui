use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};
use framework_tui::{
  CommandCompletion, CommandState, KeyBindings, KeyContext, KeyDispatcher, KeyHint, MatchResult,
  Prompt, current_word_start, filter_completion_candidates, key_event_to_token,
};
use ratatui::layout::Rect;
use tokio::sync::mpsc;

use crate::{
  cache,
  config::{self, EffectiveLayoutConfig, Settings},
  event::{AsyncEvent, CacheClearOutcome},
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

  pub fn set_user_progress_target(&mut self, progress: f64) {
    self.set_progress_target(progress);
  }

  pub fn clear_cached_images(&mut self) {
    self.pages.fill(None);
    self.slices.clear();
    self.page_errors.fill(None);
    self.slice_errors.clear();
  }

  pub fn current_progress(&self) -> Option<f64> {
    if self.document.page_count == 0 {
      return Some(0.0);
    }
    if self.layout.is_scroll() {
      let layout = self.last_scroll_layout.as_ref()?;
      return progress_for_scroll_row(
        layout,
        self.scroll as usize,
        self.viewport_height,
        self.layout.scroll_divisor,
      );
    }
    progress_for_grid_start(
      self.grid_start_page,
      self.layout.grid_capacity(),
      self.document.page_count,
    )
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

  pub fn handle_input(&mut self, input: Event, tx: &mpsc::UnboundedSender<AsyncEvent>) -> bool {
    let force_redraw = matches!(input, Event::Resize(_, _));
    let before = self.input_redraw_state();
    match input {
      Event::Key(key) if self.prompt.is_some() => self.handle_prompt_key(key, tx),
      Event::Paste(value) if self.prompt.is_some() => self.handle_prompt_paste(&value),
      Event::Key(key) => {
        let Some(token) = key_event_to_token(key) else {
          return false;
        };
        self.handle_key_token(token, tx);
      }
      Event::Mouse(mouse) => match mouse.kind {
        MouseEventKind::ScrollDown => self.scroll_down(),
        MouseEventKind::ScrollUp => self.scroll_up(),
        _ => {}
      },
      Event::Resize(_, _) => {}
      _ => {}
    }
    force_redraw || before != self.input_redraw_state()
  }

  fn input_redraw_state(&self) -> InputRedrawState {
    let prompt = self.prompt.as_ref().map(|prompt| PromptRedrawState {
      prefix: prompt.prefix().to_string(),
      input: prompt.buffer().input.clone(),
      cursor: prompt.buffer().cursor,
    });
    let completion = self
      .command_completion()
      .map(|completion| CompletionRedrawState {
        candidates: completion.candidates.clone(),
        selected: completion.selected,
      });
    InputRedrawState {
      scroll: self.scroll,
      grid_start_page: self.grid_start_page,
      focused_page: self.focused_page,
      layout: self.layout.label(),
      message: self.message.clone(),
      quit: self.quit,
      prompt,
      completion,
      key_hint_count: self.key_hints().len(),
    }
  }

  fn handle_key_token(&mut self, token: String, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    match self
      .key_dispatcher
      .dispatch(&self.keymap, KeyContext::Browser, token)
    {
      MatchResult::Action(action) => self.handle_action(&action, tx),
      MatchResult::Prefix(_) | MatchResult::None => {}
    }
  }

  fn handle_action(&mut self, action: &str, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    if let Some(command) = action.strip_prefix("layout-use ") {
      self.execute_layout_command(command, false);
      return;
    }
    if let Some(command) = action.strip_prefix("layout ") {
      self.execute_layout_command(command, true);
      return;
    }

    match action {
      "quit" => self.quit = true,
      "command" => self.start_command(),
      "scroll_down" => self.scroll_down(),
      "scroll_up" => self.scroll_up(),
      "page_down" => self.page_down(),
      "page_up" => self.page_up(),
      "next_page" => self.next_page(),
      "previous_page" => self.previous_page(),
      "home" => self.home(),
      "end" => self.end(),
      "clear-cache" | "clear_cache" => self.request_clear_cache(tx),
      other => self.set_message(format!("unknown action: {other}")),
    }
  }

  fn start_command(&mut self) {
    self.command_state.reset_prompt_state();
    self.prompt = Some(Prompt::command(String::new()));
    self.refresh_command_completion();
  }

  fn handle_prompt_key(&mut self, key: KeyEvent, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    let Some(token) = key_event_to_token(key) else {
      return;
    };
    match self
      .key_dispatcher
      .dispatch(&self.keymap, KeyContext::Input, token)
    {
      MatchResult::Action(action) => self.handle_prompt_action(&action, tx),
      MatchResult::Prefix(_) => {}
      MatchResult::None => self.insert_prompt_key(key),
    }
  }

  fn handle_prompt_paste(&mut self, value: &str) {
    if let Some(prompt) = self.prompt.as_mut() {
      prompt.buffer_mut().insert_str(value);
      self.command_state.reset_history_cursor();
      self.refresh_command_completion();
    }
  }

  fn handle_prompt_action(&mut self, action: &str, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    match action {
      "cancel" => {
        self.prompt = None;
        self.command_state.reset_prompt_state();
        self.key_dispatcher.clear();
        self.set_message("cancelled");
      }
      "submit" => self.submit_prompt(tx),
      "backspace" => self.edit_prompt(|prompt| prompt.buffer_mut().backspace()),
      "delete" => self.edit_prompt(|prompt| prompt.buffer_mut().delete()),
      "move_left" => self.edit_prompt_no_completion(|prompt| prompt.buffer_mut().move_left()),
      "move_right" => self.edit_prompt_no_completion(|prompt| prompt.buffer_mut().move_right()),
      "move_start" => self.edit_prompt_no_completion(|prompt| prompt.buffer_mut().move_start()),
      "move_end" => self.edit_prompt_no_completion(|prompt| prompt.buffer_mut().move_end()),
      "kill_before_cursor" => self.edit_prompt(|prompt| prompt.buffer_mut().kill_before_cursor()),
      "kill_after_cursor" => self.edit_prompt(|prompt| prompt.buffer_mut().kill_after_cursor()),
      "completion_next" => self.select_next_completion(),
      "completion_previous" => self.select_previous_completion(),
      "history_previous" => self.history_previous(),
      "history_next" => self.history_next(),
      _ => {}
    }
  }

  fn insert_prompt_key(&mut self, key: KeyEvent) {
    if key.kind != KeyEventKind::Press
      || key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
      return;
    }
    let KeyCode::Char(ch) = key.code else {
      return;
    };
    self.edit_prompt(|prompt| prompt.buffer_mut().insert_char(ch));
  }

  fn submit_prompt(&mut self, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    if self.complete_selected_command_candidate() {
      return;
    }
    let Some(prompt) = self.prompt.take() else {
      return;
    };
    let command = prompt.buffer().input.trim().to_string();
    self.command_state.reset_prompt_state();
    self.key_dispatcher.clear();
    if command.is_empty() {
      return;
    }
    self.command_state.push_history(command.clone());
    self.execute_command(&command, tx);
  }

  fn edit_prompt(&mut self, edit: impl FnOnce(&mut Prompt)) {
    if let Some(prompt) = self.prompt.as_mut() {
      edit(prompt);
      self.command_state.reset_history_cursor();
      self.refresh_command_completion();
    }
  }

  fn edit_prompt_no_completion(&mut self, edit: impl FnOnce(&mut Prompt)) {
    if let Some(prompt) = self.prompt.as_mut() {
      edit(prompt);
    }
    self.refresh_command_completion();
  }

  fn complete_selected_command_candidate(&mut self) -> bool {
    self.refresh_command_completion();
    let Some(prompt) = self.prompt.as_mut() else {
      return false;
    };
    let changed = self.command_state.apply_completion(prompt.buffer_mut());
    if changed {
      self.refresh_command_completion();
    }
    changed
  }

  fn select_next_completion(&mut self) {
    self.refresh_command_completion();
    self.command_state.select_next_completion();
  }

  fn select_previous_completion(&mut self) {
    self.refresh_command_completion();
    self.command_state.select_previous_completion();
  }

  fn history_previous(&mut self) {
    if let Some(prompt) = self.prompt.as_mut() {
      self.command_state.history_previous(prompt.buffer_mut());
      self.refresh_command_completion();
    }
  }

  fn history_next(&mut self) {
    if let Some(prompt) = self.prompt.as_mut() {
      self.command_state.history_next(prompt.buffer_mut());
      self.refresh_command_completion();
    }
  }

  fn refresh_command_completion(&mut self) {
    let Some(prompt) = &self.prompt else {
      self.command_state.clear_completion();
      return;
    };
    if !prompt.is_command() {
      self.command_state.clear_completion();
      return;
    }
    let buffer = prompt.buffer();
    let completion = self.command_completion_for(&buffer.input, buffer.cursor);
    self
      .command_state
      .set_completion_preserving_selection(completion);
  }

  fn command_completion_for(&self, input: &str, cursor: usize) -> Option<CommandCompletion> {
    let cursor = cursor.min(input.len());
    let before_cursor = input.get(..cursor)?;
    let normalized = before_cursor.trim_start_matches(':');
    let tokens = normalized.split_whitespace().collect::<Vec<_>>();
    let ends_with_space = normalized.chars().last().is_some_and(char::is_whitespace);
    let word_start = current_word_start(input, cursor);
    let prefix = if ends_with_space {
      ""
    } else {
      input.get(word_start..cursor).unwrap_or_default()
    };

    if tokens.is_empty() || (tokens.len() == 1 && !ends_with_space) {
      return Some(CommandCompletion::new(
        word_start,
        cursor,
        prefix,
        filter_completion_candidates(COMMAND_NAMES.iter().copied(), prefix),
        true,
        0,
      ));
    }

    match tokens[0] {
      "layout" | "layout-use" => {
        if tokens.len() > 2 || (tokens.len() == 2 && ends_with_space) {
          return None;
        }
        let replace_start = if ends_with_space { cursor } else { word_start };
        let prefix = if ends_with_space { "" } else { prefix };
        Some(CommandCompletion::new(
          replace_start,
          cursor,
          prefix,
          filter_completion_candidates(self.settings.config.layout.presets.keys(), prefix),
          true,
          0,
        ))
      }
      _ => None,
    }
  }

  fn execute_command(&mut self, command: &str, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    let mut parts = command.split_whitespace().collect::<Vec<_>>();
    if parts.is_empty() {
      return;
    }
    match parts[0] {
      "q" | "quit" => self.quit = true,
      "layout" => {
        parts.remove(0);
        self.execute_layout_parts(&parts, true);
      }
      "layout-use" | "layout_use" => {
        parts.remove(0);
        self.execute_layout_parts(&parts, false);
      }
      "write-config" | "write_config" => {
        match config::write_app_config_sync(&self.settings.config_path, &self.settings.config) {
          Ok(()) => self.set_message(format!("wrote {}", self.settings.config_path.display())),
          Err(error) => self.set_message(format!("write-config failed: {error}")),
        }
      }
      "clear-cache" | "clear_cache" => {
        if parts.len() > 1 {
          self.set_message("usage: clear-cache");
        } else {
          self.request_clear_cache(tx);
        }
      }
      "help" => self.set_message("commands: layout, layout-use, write-config, clear-cache, quit"),
      other => self.set_message(format!("unknown command: {other}")),
    }
  }

  fn request_clear_cache(&mut self, tx: &mpsc::UnboundedSender<AsyncEvent>) {
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

  fn execute_layout_command(&mut self, command: &str, persist: bool) {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    self.execute_layout_parts(&parts, persist);
  }

  fn execute_layout_parts(&mut self, parts: &[&str], persist: bool) {
    let Some((name, args)) = parts.split_first() else {
      self.set_message("usage: layout <scroll|grid> ...");
      return;
    };
    let preserved_progress = self.current_progress().or(self.pending_progress);
    let result = if persist {
      self.settings.config.layout.set_active_from_args(name, args)
    } else {
      let mut layout = self.settings.config.layout.clone();
      layout.set_active_from_args(name, args)
    };
    match result {
      Ok(layout) => {
        self.layout = layout;
        self.scroll = 0;
        self.grid_start_page = 0;
        self.focused_page = 0;
        if let Some(progress) = preserved_progress {
          self.set_progress_target(progress);
        } else {
          self.normalize_current_layout_state();
        }
        if persist {
          match config::write_app_config_sync(&self.settings.config_path, &self.settings.config) {
            Ok(()) => self.set_message(format!("layout saved: {}", self.layout.label())),
            Err(error) => self.set_message(format!("layout changed, save failed: {error}")),
          }
        } else {
          self.set_message(format!("layout use: {}", self.layout.label()));
        }
      }
      Err(error) => self.set_message(error),
    }
  }

  fn scroll_down(&mut self) {
    if self.layout.is_scroll() {
      self.scroll_by_rows(1);
    } else {
      self.shift_grid_window(self.grid_row_step());
    }
  }

  fn scroll_up(&mut self) {
    if self.layout.is_scroll() {
      self.scroll_by_rows(-1);
    } else {
      self.shift_grid_window(-self.grid_row_step());
    }
  }

  fn page_down(&mut self) {
    if self.layout.is_scroll() {
      self.scroll_by_rows(i32::from(self.layout.scroll_divisor.max(1)));
    } else {
      self.shift_grid_window(self.grid_capacity_step());
    }
  }

  fn page_up(&mut self) {
    if self.layout.is_scroll() {
      self.scroll_by_rows(-i32::from(self.layout.scroll_divisor.max(1)));
    } else {
      self.shift_grid_window(-self.grid_capacity_step());
    }
  }

  fn next_page(&mut self) {
    self.focus_relative(1);
  }

  fn previous_page(&mut self) {
    self.focus_relative(-1);
  }

  fn home(&mut self) {
    self.focused_page = 0;
    self.scroll = 0;
    self.grid_start_page = 0;
  }

  fn end(&mut self) {
    if self.document.page_count == 0 {
      self.focused_page = 0;
      self.scroll = 0;
      return;
    }
    self.focused_page = self.document.page_count - 1;
    if self.layout.is_scroll() {
      self.scroll_to_focused_page();
    } else {
      self.grid_start_page = self.grid_max_start(self.layout.grid_capacity());
    }
  }

  fn focus_relative(&mut self, delta: isize) {
    if self.document.page_count == 0 {
      self.focused_page = 0;
      self.grid_start_page = 0;
      return;
    }
    if !self.layout.is_scroll() {
      self.shift_grid_window(delta);
      return;
    }
    self.focused_page = self
      .focused_page
      .saturating_add_signed(delta)
      .min(self.document.page_count - 1);
    self.scroll_to_focused_page();
  }

  fn grid_row_step(&self) -> isize {
    isize::try_from(self.layout.columns.max(1)).unwrap_or(isize::MAX)
  }

  fn grid_capacity_step(&self) -> isize {
    isize::try_from(self.layout.grid_capacity().max(1)).unwrap_or(isize::MAX)
  }

  fn shift_grid_window(&mut self, delta_pages: isize) {
    if self.document.page_count == 0 {
      self.focused_page = 0;
      self.grid_start_page = 0;
      return;
    }
    let capacity = self.layout.grid_capacity().max(1);
    self.clamp_grid_start(capacity);
    let max_start = self.grid_max_start(capacity);
    self.grid_start_page = self
      .grid_start_page
      .saturating_add_signed(delta_pages)
      .min(max_start);
    self.focused_page = self.grid_start_page.min(self.document.page_count - 1);
  }

  fn clamp_grid_start(&mut self, capacity: usize) {
    self.grid_start_page = self.grid_start_page.min(self.grid_max_start(capacity));
  }

  fn grid_max_start(&self, capacity: usize) -> usize {
    self.document.page_count.saturating_sub(capacity.max(1))
  }

  fn scroll_by_rows(&mut self, delta: i32) {
    let max_scroll = self.max_scroll();
    self.scroll = self.scroll.saturating_add_signed(delta).min(max_scroll);
    self.update_focus_from_scroll();
  }

  fn scroll_to_focused_page(&mut self) {
    let Some(layout) = &self.last_scroll_layout else {
      return;
    };
    let Some((row_index, _)) = layout.rows.iter().enumerate().find(|(_, row)| {
      row.items.iter().any(|item| {
        layout
          .items
          .get(*item)
          .is_some_and(|item| item.page_index == self.focused_page)
      })
    }) else {
      return;
    };
    self.scroll = (row_index as u32).min(self.max_scroll());
  }

  fn update_focus_from_scroll(&mut self) {
    let Some(layout) = &self.last_scroll_layout else {
      return;
    };
    let Some(row) = layout.rows.get(self.scroll as usize) else {
      return;
    };
    if let Some(page_index) = row
      .items
      .iter()
      .filter_map(|item| layout.items.get(*item))
      .map(|item| item.page_index)
      .min()
    {
      self.focused_page = page_index;
    }
  }

  fn max_scroll(&self) -> u32 {
    self
      .last_scroll_layout
      .as_ref()
      .map(|layout| {
        crate::layout::max_scroll_row_for_viewport(
          layout,
          self.viewport_height,
          self.layout.scroll_divisor,
        ) as u32
      })
      .unwrap_or(0)
  }

  fn set_progress_target(&mut self, progress: f64) {
    let progress = self.clamp_progress(progress);
    if self.apply_progress_to_current_layout(progress) {
      self.pending_progress = None;
    } else {
      self.pending_progress = Some(progress);
    }
  }

  fn apply_pending_progress_if_ready(&mut self) {
    let Some(progress) = self.pending_progress else {
      return;
    };
    if self.apply_progress_to_current_layout(progress) {
      self.pending_progress = None;
    }
  }

  fn apply_progress_to_current_layout(&mut self, progress: f64) -> bool {
    if self.document.page_count == 0 {
      self.scroll = 0;
      self.grid_start_page = 0;
      self.focused_page = 0;
      return true;
    }
    if self.layout.is_scroll() {
      let Some(layout) = &self.last_scroll_layout else {
        return false;
      };
      self.scroll = best_scroll_row_for_progress(
        layout,
        self.viewport_height,
        self.layout.scroll_divisor,
        progress,
      ) as u32;
      self.update_focus_from_scroll();
      true
    } else {
      let capacity = self.layout.grid_capacity().max(1);
      self.grid_start_page = best_grid_start_for_progress(
        progress,
        capacity,
        self.layout.columns.max(1) as usize,
        self.document.page_count,
      );
      self.focused_page = self.grid_start_page.min(self.document.page_count - 1);
      true
    }
  }

  fn normalize_current_layout_state(&mut self) {
    if self.document.page_count == 0 {
      self.scroll = 0;
      self.grid_start_page = 0;
      self.focused_page = 0;
      return;
    }
    if self.layout.is_scroll() {
      self.grid_start_page = 0;
      self.scroll = self.scroll.min(self.max_scroll());
      self.update_focus_from_scroll();
    } else {
      let capacity = self.layout.grid_capacity();
      self.clamp_grid_start(capacity);
      self.focused_page = self.grid_start_page.min(self.document.page_count - 1);
    }
  }

  fn clamp_progress(&self, progress: f64) -> f64 {
    if !progress.is_finite() {
      return 0.0;
    }
    progress.clamp(0.0, self.document.page_count as f64)
  }
}

const COMMAND_NAMES: &[&str] = &[
  "clear-cache",
  "help",
  "layout",
  "layout-use",
  "quit",
  "write-config",
];

fn best_scroll_row_for_progress(
  layout: &ScrollLayout,
  viewport_height: u16,
  scroll_divisor: u16,
  target: f64,
) -> usize {
  if layout.rows.is_empty() {
    return 0;
  }
  let mut best_row = 0;
  let mut best_distance = f64::INFINITY;
  let max_row = crate::layout::max_scroll_row_for_viewport(layout, viewport_height, scroll_divisor);
  for row_index in 0..=max_row {
    let Some(progress) =
      progress_for_scroll_row(layout, row_index, viewport_height, scroll_divisor)
    else {
      continue;
    };
    let distance = (progress - target).abs();
    if distance < best_distance {
      best_row = row_index;
      best_distance = distance;
    }
  }
  best_row
}

fn progress_for_scroll_row(
  layout: &ScrollLayout,
  row_index: usize,
  viewport_height: u16,
  scroll_divisor: u16,
) -> Option<f64> {
  let visible_rows =
    crate::layout::visible_scroll_rows(layout, row_index, viewport_height, scroll_divisor);
  let mut weighted_sum = 0.0;
  let mut total_weight = 0.0;
  for row_index in visible_rows {
    let Some(row) = layout.rows.get(row_index) else {
      continue;
    };
    for item_index in &row.items {
      let Some(item) = layout.items.get(*item_index) else {
        continue;
      };
      let full_width = f64::from(item.full_width.max(1));
      let full_height = f64::from(item.full_height.max(1));
      let slice_count = item.slice_count.max(1);
      let top_cells =
        (u32::from(item.full_height) * u32::from(item.slice_index)) / u32::from(slice_count);
      let bottom_cells = (u32::from(item.full_height)
        * u32::from(item.slice_index.saturating_add(1)))
        / u32::from(slice_count);
      let top = f64::from(top_cells) / full_height;
      let bottom = f64::from(bottom_cells) / full_height;
      let width_fraction = f64::from(item.width.max(1)) / full_width;
      let height_fraction = (bottom - top).max(0.0);
      let weight = width_fraction * height_fraction;
      if weight <= 0.0 {
        continue;
      }
      let progress = item.page_index as f64 + (top + bottom) / 2.0;
      weighted_sum += progress * weight;
      total_weight += weight;
    }
  }
  (total_weight > 0.0).then_some(weighted_sum / total_weight)
}

fn best_grid_start_for_progress(
  target: f64,
  capacity: usize,
  row_step: usize,
  page_count: usize,
) -> usize {
  if page_count == 0 {
    return 0;
  }
  let mut best_start = 0;
  let mut best_distance = f64::INFINITY;
  for start in reachable_grid_starts(page_count, capacity, row_step) {
    let Some(progress) = progress_for_grid_start(start, capacity, page_count) else {
      continue;
    };
    let distance = (progress - target).abs();
    if distance < best_distance {
      best_start = start;
      best_distance = distance;
    }
  }
  best_start
}

fn reachable_grid_starts(page_count: usize, capacity: usize, row_step: usize) -> Vec<usize> {
  if page_count == 0 {
    return vec![0];
  }
  let capacity = capacity.max(1);
  let row_step = row_step.max(1);
  let max_start = page_count.saturating_sub(capacity);
  let mut starts = Vec::new();
  let mut start = 0;
  loop {
    let clamped = start.min(max_start);
    if starts.last().copied() != Some(clamped) {
      starts.push(clamped);
    }
    if clamped == max_start {
      break;
    }
    start = start.saturating_add(row_step);
  }
  starts
}

fn progress_for_grid_start(start: usize, capacity: usize, page_count: usize) -> Option<f64> {
  if page_count == 0 {
    return Some(0.0);
  }
  let start = start.min(page_count.saturating_sub(1));
  let visible_count = page_count.saturating_sub(start).min(capacity.max(1));
  if visible_count == 0 {
    return None;
  }
  let first = start as f64 + 0.5;
  let last = start.saturating_add(visible_count - 1) as f64 + 0.5;
  Some((first + last) / 2.0)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn grid_reachable_starts_match_row_browsing_with_clamped_end() {
    assert_eq!(reachable_grid_starts(20, 6, 3), vec![0, 3, 6, 9, 12, 14]);
    assert_eq!(reachable_grid_starts(10, 6, 3), vec![0, 3, 4]);
    assert_eq!(reachable_grid_starts(3, 6, 3), vec![0]);
  }

  #[test]
  fn grid_progress_lookup_only_chooses_reachable_starts() {
    assert_eq!(best_grid_start_for_progress(0.0, 6, 3, 20), 0);
    assert_eq!(best_grid_start_for_progress(6.0, 6, 3, 20), 3);
    assert_eq!(best_grid_start_for_progress(18.0, 6, 3, 20), 14);
  }

  #[test]
  fn grid_progress_is_zero_based() {
    assert_eq!(progress_for_grid_start(0, 1, 10), Some(0.5));
    assert_eq!(best_grid_start_for_progress(0.0, 1, 1, 10), 0);
  }
}
