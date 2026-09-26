//! Routing terminal input: keys and mouse events become actions of the
//! current view, the prompt, or an open dialog.

use crossterm::event::{Event, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use framework_tui::{
  MatchResult, PromptInputResult, handle_prompt_key as framework_handle_prompt_key,
  handle_prompt_paste as framework_handle_prompt_paste, key_event_to_token,
};
use ratatui::layout::Rect;
use tokio::sync::mpsc;

use crate::{
  event::AsyncEvent,
  geometry::{contains, safe_inner, split_panels},
};

use super::{App, ViewMode};

/// Everything input can change that is visible on screen. Comparing it
/// before and after an event decides whether a redraw is needed.
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
  viewer_search_highlight: bool,
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
  pub fn handle_input(&mut self, input: Event, tx: &mpsc::UnboundedSender<AsyncEvent>) -> bool {
    let force_redraw = matches!(input, Event::Resize(_, _));
    let before = self.input_redraw_state();
    if self.key_help {
      self.handle_key_help_input(input);
      return force_redraw || before != self.input_redraw_state();
    }
    if self.confirm.is_some() {
      self.handle_confirm_input(input, tx);
      return force_redraw || before != self.input_redraw_state();
    }
    match input {
      Event::Key(key) if self.prompt.is_some() => self.handle_prompt_key(key, tx),
      Event::Paste(value) if self.prompt.is_some() => self.handle_prompt_paste(&value),
      Event::Key(key) if self.view == ViewMode::Search => self.handle_search_key(key, tx),
      Event::Paste(value) if self.view == ViewMode::Search => self.handle_search_paste(&value, tx),
      Event::Key(key) => {
        let Some(token) = key_event_to_token(key) else {
          return false;
        };
        self.handle_key_token(token, tx);
      }
      Event::Mouse(mouse) => match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) if self.view == ViewMode::Bookmarks => {
          self.handle_bookmarks_mouse_click(mouse)
        }
        MouseEventKind::Down(MouseButton::Left) if self.view == ViewMode::Search => {
          self.handle_search_mouse_click(mouse)
        }
        MouseEventKind::Down(button)
          if matches!(self.view, ViewMode::Viewer | ViewMode::Selection) =>
        {
          self.handle_selection_mouse_down(mouse, button)
        }
        MouseEventKind::Drag(button)
          if matches!(self.view, ViewMode::Viewer | ViewMode::Selection) =>
        {
          self.handle_selection_mouse_drag(mouse, button)
        }
        MouseEventKind::Up(button)
          if matches!(self.view, ViewMode::Viewer | ViewMode::Selection) =>
        {
          self.handle_selection_mouse_up(mouse, button, tx)
        }
        MouseEventKind::ScrollDown if self.view == ViewMode::Metadata => {
          self.metadata_scroll_down()
        }
        MouseEventKind::ScrollUp if self.view == ViewMode::Metadata => self.metadata_scroll_up(),
        MouseEventKind::ScrollDown if self.view == ViewMode::Bookmarks => {
          self.handle_frame_navigation(|app| app.bookmarks.select_visible_delta(1))
        }
        MouseEventKind::ScrollUp if self.view == ViewMode::Bookmarks => {
          self.handle_frame_navigation(|app| app.bookmarks.select_visible_delta(-1))
        }
        MouseEventKind::ScrollDown if self.view == ViewMode::Search => {
          self.handle_frame_navigation(|app| app.select_search_delta(1))
        }
        MouseEventKind::ScrollUp if self.view == ViewMode::Search => {
          self.handle_frame_navigation(|app| app.select_search_delta(-1))
        }
        MouseEventKind::ScrollDown if self.view == ViewMode::Selection => self.selection_next(),
        MouseEventKind::ScrollUp if self.view == ViewMode::Selection => self.selection_previous(),
        MouseEventKind::ScrollDown => self.handle_frame_navigation(|app| app.scroll_down()),
        MouseEventKind::ScrollUp => self.handle_frame_navigation(|app| app.scroll_up()),
        _ => {}
      },
      Event::Resize(_, _) => {}
      _ => {}
    }
    force_redraw || before != self.input_redraw_state()
  }

  /// A click selects a bookmark; clicking the selected one toggles it.
  fn handle_bookmarks_mouse_click(&mut self, mouse: MouseEvent) {
    self.handle_frame_navigation(|app| {
      let Some(index) = app.bookmark_index_at(mouse.column, mouse.row) else {
        return;
      };
      if app.bookmarks.selected == Some(index) {
        app.bookmarks.toggle_selected();
      } else {
        app.bookmarks.selected = Some(index);
      }
    });
  }

  /// A click selects a result; clicking the selected one opens it.
  fn handle_search_mouse_click(&mut self, mouse: MouseEvent) {
    self.handle_frame_navigation(|app| {
      let Some(index) = app.search_result_index_at(mouse.column, mouse.row) else {
        return;
      };
      if app.search.selected == Some(index) {
        app.search_open();
      } else {
        app.search.selected = Some(index);
        app.search.make_preload_ready_now();
      }
    });
  }

  fn bookmark_index_at(&self, column: u16, row: u16) -> Option<usize> {
    if self.bookmarks.error.is_some() || self.bookmarks.entries.is_empty() {
      return None;
    }
    let (tree, _) = split_panels(
      self.viewport?,
      self.bookmarks.left_ratio,
      self.bookmarks.right_ratio,
    );
    let inner = safe_inner(tree, 1, 1);
    if !contains(inner, column, row) {
      return None;
    }
    self
      .bookmarks
      .index_at_row(usize::from(row.saturating_sub(inner.y)))
  }

  fn search_result_index_at(&self, column: u16, row: u16) -> Option<usize> {
    let search = &self.search;
    if search.query().is_empty()
      || search.index_loading
      || search.index_error.is_some()
      || search.results.is_empty()
    {
      return None;
    }
    let (panel, _) = split_panels(self.viewport?, search.left_ratio, search.right_ratio);
    let inner = safe_inner(panel, 1, 1);
    if inner.height <= 1 {
      return None;
    }
    let results = Rect {
      x: inner.x,
      y: inner.y.saturating_add(1),
      width: inner.width,
      height: inner.height.saturating_sub(1),
    };
    if !contains(results, column, row) {
      return None;
    }
    let row_offset = row.saturating_sub(results.y) as usize;
    search.result_at_row(row_offset)
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
      view: self.view,
      metadata_scroll: self.metadata_scroll,
      bookmarks_selected: self.bookmarks.selected,
      bookmarks_scroll: self.bookmarks.scroll,
      bookmarks_expanded_len: self.bookmarks.expanded.len(),
      bookmarks_left_ratio: self.bookmarks.left_ratio,
      bookmarks_right_ratio: self.bookmarks.right_ratio,
      search_input: self.search.prompt.buffer().input.clone(),
      search_cursor: self.search.prompt.buffer().cursor,
      search_results_len: self.search.results.len(),
      search_selected: self.search.selected,
      viewer_search_highlight: self.search.viewer_highlight.is_some(),
      search_scroll: self.search.scroll,
      search_index_loading: self.search.index_loading,
      search_index_error: self.search.index_error.clone(),
      selection_anchor_active: self.selection.anchor.is_some(),
      selection_anchor_state: self.selection_anchor_state(),
      selections_len: self.selection.history.len(),
      selection_index: self.selection.index,
      selection_copy_text_pending: self.selection.copy_text_pending,
      selection_copy_image_pending: self.selection.copy_image_pending,
      confirm: self.confirm.is_some(),
      key_help: self.key_help,
      editor_request: self.editor_request.is_some(),
      layout: self.layout.label(),
      message: self.message.clone(),
      frame_navigation_locked: self.frame_navigation_locked,
      quit: self.quit,
      prompt,
      completion,
      key_hint_count: self.key_hints().len(),
    }
  }

  fn handle_key_token(&mut self, token: String, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    let result = if self.view == ViewMode::Bookmarks {
      self
        .key_dispatcher
        .dispatch(&self.bookmarks_keymap, self.key_context(), token)
    } else if self.view == ViewMode::Search {
      self
        .key_dispatcher
        .dispatch(&self.search_keymap, self.key_context(), token)
    } else if self.view == ViewMode::Selection {
      self
        .key_dispatcher
        .dispatch(&self.selection_keymap, self.key_context(), token)
    } else {
      self
        .key_dispatcher
        .dispatch(&self.keymap, self.key_context(), token)
    };
    match result {
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
      "back" if self.selection.anchor.is_some() => self.cancel_selection_anchor(),
      "back" => self.back_to_viewer(),
      "command" => self.start_command(),
      "help" => self.show_key_help(),
      "scroll_down" if self.view == ViewMode::Metadata => self.metadata_scroll_down(),
      "scroll_up" if self.view == ViewMode::Metadata => self.metadata_scroll_up(),
      "page_down" if self.view == ViewMode::Metadata => self.metadata_page_down(),
      "page_up" if self.view == ViewMode::Metadata => self.metadata_page_up(),
      "scroll_down" => self.handle_frame_navigation(|app| app.scroll_down()),
      "scroll_up" => self.handle_frame_navigation(|app| app.scroll_up()),
      "page_down" => self.handle_frame_navigation(|app| app.page_down()),
      "page_up" => self.handle_frame_navigation(|app| app.page_up()),
      "next_page" => self.handle_frame_navigation(|app| app.next_page()),
      "previous_page" => self.handle_frame_navigation(|app| app.previous_page()),
      "home" => self.handle_frame_navigation(|app| app.home()),
      "end" => self.handle_frame_navigation(|app| app.end()),
      "clear-cache" | "clear_cache" => self.request_clear_cache(tx),
      "refresh" => self.request_refresh(tx),
      "metadata" => self.enter_metadata_view(),
      "bookmarks" => self.enter_bookmarks_view(),
      "search" => self.enter_search_view(tx),
      "selection" => self.enter_selection_view(),
      "edit_metadata" => self.start_metadata_edit(),
      "edit_bookmarks" => self.start_bookmarks_edit(),
      "metadata_scroll_down" => self.metadata_scroll_down(),
      "metadata_scroll_up" => self.metadata_scroll_up(),
      "metadata_page_down" => self.metadata_page_down(),
      "metadata_page_up" => self.metadata_page_up(),
      "bookmarks_next" => self.handle_frame_navigation(|app| app.bookmarks.select_visible_delta(1)),
      "bookmarks_previous" => {
        self.handle_frame_navigation(|app| app.bookmarks.select_visible_delta(-1))
      }
      "bookmarks_page_down" => self.handle_frame_navigation(|app| {
        let step = app.bookmarks_page_step();
        app.bookmarks.select_visible_delta(step)
      }),
      "bookmarks_page_up" => self.handle_frame_navigation(|app| {
        let step = app.bookmarks_page_step();
        app.bookmarks.select_visible_delta(-step)
      }),
      "bookmarks_toggle" => self.bookmarks.toggle_selected(),
      "bookmarks_toggle_all" => self.bookmarks.toggle_all(),
      "bookmarks_open" => self.handle_frame_navigation(|app| app.bookmarks_open()),
      "bookmarks_panel_narrower" => self.bookmarks.narrow_panel(),
      "bookmarks_panel_wider" => self.bookmarks.widen_panel(),
      "search_next" => self.handle_frame_navigation(|app| app.select_search_delta(1)),
      "search_previous" => self.handle_frame_navigation(|app| app.select_search_delta(-1)),
      "search_page_down" => self.handle_frame_navigation(|app| {
        let step = app.search_page_step();
        app.select_search_delta(step)
      }),
      "search_page_up" => self.handle_frame_navigation(|app| {
        let step = app.search_page_step();
        app.select_search_delta(-step)
      }),
      "search_open" => self.handle_frame_navigation(|app| app.search_open()),
      "selection_mark" => self.set_message("selection mark requires a mouse position"),
      "selection_cancel" if self.selection.anchor.is_some() => self.cancel_selection_anchor(),
      "selection_cancel" if self.view == ViewMode::Selection => self.back_to_viewer(),
      "selection_cancel" => self.cancel_selection_anchor(),
      "selection_next" => self.selection_next(),
      "selection_previous" => self.selection_previous(),
      "selection_reselect" => self.selection_reselect(),
      "selection_copy_text" => self.selection_copy_text(tx),
      "selection_copy_image" => self.selection_copy_image(tx),
      other => self.set_message(format!("unknown action: {other}")),
    }
  }

  fn handle_selection_mouse_down(&mut self, mouse: MouseEvent, button: MouseButton) {
    let token = mouse_button_token(button).to_string();
    let result = if self.view == ViewMode::Selection {
      self
        .selection_keymap
        .match_sequence(self.key_context(), &[token])
    } else {
      self.keymap.match_sequence(self.key_context(), &[token])
    };
    if matches!(result, MatchResult::Action(action) if action == "selection_mark") {
      self.begin_selection_mouse_press(mouse, button);
    }
  }

  fn handle_selection_mouse_up(
    &mut self,
    mouse: MouseEvent,
    button: MouseButton,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) {
    if self.finish_selection_mouse_press(mouse, button, tx) {
      return;
    }
    let token = mouse_button_token(button);
    let result = if self.view == ViewMode::Selection {
      self
        .key_dispatcher
        .dispatch(&self.selection_keymap, self.key_context(), token)
    } else {
      self
        .key_dispatcher
        .dispatch(&self.keymap, self.key_context(), token)
    };
    match result {
      MatchResult::Action(action)
        if action == "selection_mark" && self.selection.anchor.is_some() =>
      {
        self.handle_selection_mouse_click(mouse, tx)
      }
      MatchResult::Action(action) => self.handle_action(&action, tx),
      MatchResult::Prefix(_) | MatchResult::None => {}
    }
  }

  fn handle_frame_navigation(&mut self, navigate: impl FnOnce(&mut Self)) {
    if self.frame_sync_navigation_blocks() {
      return;
    }
    let before = self.frame_navigation_state();
    let clear_search_highlight = self.view == ViewMode::Viewer;
    navigate(self);
    if clear_search_highlight && self.view == ViewMode::Viewer {
      self.search.viewer_highlight = None;
    }
    if self.frame_sync_navigation_enabled() && before != self.frame_navigation_state() {
      self.lock_frame_navigation_if_enabled();
    }
  }

  fn frame_sync_navigation_blocks(&self) -> bool {
    self.frame_sync_navigation_enabled() && self.frame_navigation_locked
  }

  fn frame_navigation_state(&self) -> (ViewMode, u32, usize, usize, Option<usize>, Option<usize>) {
    (
      self.view,
      self.scroll,
      self.grid_start_page,
      self.focused_page,
      self.bookmarks.selected,
      self.search.selected,
    )
  }

  fn handle_search_key(&mut self, key: KeyEvent, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    if let Some(token) = key_event_to_token(key) {
      match self
        .key_dispatcher
        .dispatch(&self.search_keymap, self.key_context(), token)
      {
        MatchResult::Action(action) => {
          self.handle_action(&action, tx);
          return;
        }
        MatchResult::Prefix(_) => return,
        MatchResult::None => {}
      }
    }

    let before = self.search.prompt.buffer().input.clone();
    let result = framework_handle_prompt_key(
      &mut self.search.prompt,
      &mut self.search.command_state,
      &self.keymap,
      key,
    );
    match result {
      PromptInputResult::Changed => {
        if self.search.prompt.buffer().input != before {
          self.refresh_search_results();
          self.defer_search_preload_after_input(tx);
        }
      }
      PromptInputResult::UnknownAction(action) if action == "help" => self.show_key_help(),
      PromptInputResult::UnknownAction(action) => {
        self.set_message(format!("unknown search input action: {action}"));
      }
      PromptInputResult::Cancel => self.back_to_viewer(),
      PromptInputResult::Submit
      | PromptInputResult::Unhandled
      | PromptInputResult::EditInEditor { .. } => {}
    }
  }

  fn handle_search_paste(&mut self, value: &str, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    let before = self.search.prompt.buffer().input.clone();
    let result = framework_handle_prompt_paste(
      &mut self.search.prompt,
      &mut self.search.command_state,
      value,
    );
    if result == PromptInputResult::Changed && self.search.prompt.buffer().input != before {
      self.refresh_search_results();
      self.defer_search_preload_after_input(tx);
    }
  }

  fn handle_confirm_input(&mut self, input: Event, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    let Event::Key(key) = input else {
      return;
    };
    let Some(token) = key_event_to_token(key) else {
      return;
    };
    match token.as_str() {
      "y" => self.apply_confirm(tx),
      "enter" | "n" | "q" | "esc" => {
        self.confirm = None;
        self.set_message("cancelled");
      }
      _ => {}
    }
  }

  fn handle_key_help_input(&mut self, input: Event) {
    if let Event::Key(key) = input {
      let Some(token) = key_event_to_token(key) else {
        return;
      };
      match token.as_str() {
        "f1" | "enter" | "esc" | "q" => {
          self.key_help = false;
          self.set_message("closed key bindings");
        }
        _ => {}
      }
    }
  }

  fn back_to_viewer(&mut self) {
    self.view = ViewMode::Viewer;
    self.metadata_scroll = 0;
    self.selection.leave_view();
    self.key_dispatcher.clear();
    self.lock_frame_navigation_if_enabled();
    self.set_message("ready");
  }
}

fn mouse_button_token(button: MouseButton) -> &'static str {
  match button {
    MouseButton::Left => "mouse_left",
    MouseButton::Right => "mouse_right",
    MouseButton::Middle => "mouse_middle",
  }
}
