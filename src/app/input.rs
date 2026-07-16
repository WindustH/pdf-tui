use std::path::PathBuf;

use crossterm::event::{Event, KeyEvent, MouseEventKind};
use framework_tui::{
  CommandCompletion, MatchResult, Prompt, PromptInputResult, current_word_start,
  filter_completion_candidates, handle_prompt_key as framework_handle_prompt_key,
  handle_prompt_paste as framework_handle_prompt_paste, key_event_to_token,
};
use tokio::sync::mpsc;

use crate::{
  bookmarks, cache, config,
  event::{
    AsyncEvent, BookmarksWriteOutcome, CacheClearOutcome, DocumentReload, DocumentReloadOutcome,
    MetadataWriteOutcome,
  },
  metadata,
  pdf::PdfDocument,
};

use super::{
  App, CompletionRedrawState, ConfirmDialog, EditorRequest, InputRedrawState, PromptRedrawState,
  ViewMode,
};

const COMMAND_NAMES: &[&str] = &[
  "clear-cache",
  "bookmarks",
  "help",
  "layout",
  "layout-use",
  "metadata",
  "quit",
  "refresh",
  "write-config",
];

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
      Event::Key(key) => {
        let Some(token) = key_event_to_token(key) else {
          return false;
        };
        self.handle_key_token(token, tx);
      }
      Event::Mouse(mouse) => match mouse.kind {
        MouseEventKind::ScrollDown if self.view == ViewMode::Metadata => {
          self.metadata_scroll_down()
        }
        MouseEventKind::ScrollUp if self.view == ViewMode::Metadata => self.metadata_scroll_up(),
        MouseEventKind::ScrollDown if self.view == ViewMode::Bookmarks => self.bookmarks_next(),
        MouseEventKind::ScrollUp if self.view == ViewMode::Bookmarks => self.bookmarks_previous(),
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
      view: self.view,
      metadata_scroll: self.metadata_scroll,
      bookmarks_selected: self.bookmarks_selected,
      bookmarks_scroll: self.bookmarks_scroll,
      bookmarks_expanded_len: self.bookmarks_expanded.len(),
      bookmarks_left_ratio: self.bookmarks_left_ratio,
      bookmarks_right_ratio: self.bookmarks_right_ratio,
      confirm: self.confirm.is_some(),
      key_help: self.key_help,
      editor_request: self.editor_request.is_some(),
      layout: self.layout.label(),
      message: self.message.clone(),
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
      "back" => self.back_to_viewer(),
      "command" => self.start_command(),
      "help" => self.show_key_help(),
      "scroll_down" if self.view == ViewMode::Metadata => self.metadata_scroll_down(),
      "scroll_up" if self.view == ViewMode::Metadata => self.metadata_scroll_up(),
      "page_down" if self.view == ViewMode::Metadata => self.metadata_page_down(),
      "page_up" if self.view == ViewMode::Metadata => self.metadata_page_up(),
      "scroll_down" => self.scroll_down(),
      "scroll_up" => self.scroll_up(),
      "page_down" => self.page_down(),
      "page_up" => self.page_up(),
      "next_page" => self.next_page(),
      "previous_page" => self.previous_page(),
      "home" => self.home(),
      "end" => self.end(),
      "clear-cache" | "clear_cache" => self.request_clear_cache(tx),
      "refresh" => self.request_refresh(tx),
      "metadata" => self.enter_metadata_view(),
      "bookmarks" => self.enter_bookmarks_view(),
      "edit_metadata" => self.start_metadata_edit(),
      "edit_bookmarks" => self.start_bookmarks_edit(),
      "metadata_scroll_down" => self.metadata_scroll_down(),
      "metadata_scroll_up" => self.metadata_scroll_up(),
      "metadata_page_down" => self.metadata_page_down(),
      "metadata_page_up" => self.metadata_page_up(),
      "bookmarks_next" => self.bookmarks_next(),
      "bookmarks_previous" => self.bookmarks_previous(),
      "bookmarks_page_down" => self.bookmarks_page_down(),
      "bookmarks_page_up" => self.bookmarks_page_up(),
      "bookmarks_toggle" => self.bookmarks_toggle(),
      "bookmarks_toggle_all" => self.bookmarks_toggle_all(),
      "bookmarks_open" => self.bookmarks_open(),
      "bookmarks_panel_narrower" => self.bookmarks_panel_narrower(),
      "bookmarks_panel_wider" => self.bookmarks_panel_wider(),
      other => self.set_message(format!("unknown action: {other}")),
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
    match input {
      Event::Key(key) => {
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
      _ => {}
    }
  }

  fn apply_confirm(&mut self, tx: &mpsc::UnboundedSender<AsyncEvent>) {
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

  fn start_command(&mut self) {
    self.command_state.reset_prompt_state();
    self.prompt = Some(Prompt::command(String::new()));
    self.refresh_command_completion();
  }

  fn handle_prompt_key(&mut self, key: KeyEvent, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    let result = if let Some(prompt) = self.prompt.as_mut() {
      framework_handle_prompt_key(prompt, &mut self.command_state, &self.keymap, key)
    } else {
      PromptInputResult::Unhandled
    };
    self.handle_prompt_input_result(result, Some(tx));
  }

  fn handle_prompt_paste(&mut self, value: &str) {
    if let Some(prompt) = self.prompt.as_mut() {
      let result = framework_handle_prompt_paste(prompt, &mut self.command_state, value);
      self.handle_prompt_input_result(result, None);
    }
  }

  fn handle_prompt_input_result(
    &mut self,
    result: PromptInputResult,
    tx: Option<&mpsc::UnboundedSender<AsyncEvent>>,
  ) {
    match result {
      PromptInputResult::Unhandled => {}
      PromptInputResult::Changed => self.refresh_command_completion(),
      PromptInputResult::Cancel => self.cancel_prompt(),
      PromptInputResult::Submit => {
        if let Some(tx) = tx {
          self.submit_prompt(tx);
        }
      }
      PromptInputResult::EditInEditor { .. } => {
        self.set_message("external editor input is not supported in pdf-tui");
      }
      PromptInputResult::UnknownAction(action) if action == "help" => self.show_key_help(),
      PromptInputResult::UnknownAction(action) => {
        self.set_message(format!("unknown input action: {action}"));
      }
    }
  }

  fn cancel_prompt(&mut self) {
    self.prompt = None;
    self.command_state.reset_prompt_state();
    self.key_dispatcher.clear();
    self.set_message("cancelled");
  }

  fn submit_prompt(&mut self, tx: &mpsc::UnboundedSender<AsyncEvent>) {
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
      "metadata" => self.enter_metadata_view(),
      "bookmarks" => self.enter_bookmarks_view(),
      "refresh" => self.request_refresh(tx),
      "help" => self.show_key_help(),
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

  fn enter_metadata_view(&mut self) {
    self.view = ViewMode::Metadata;
    self.metadata_scroll = 0;
    self.key_dispatcher.clear();
    if let Some(error) = &self.metadata_error {
      self.set_message(format!("metadata unavailable: {error}"));
    } else {
      self.set_message("metadata");
    }
  }

  fn back_to_viewer(&mut self) {
    self.view = ViewMode::Viewer;
    self.metadata_scroll = 0;
    self.key_dispatcher.clear();
    self.set_message("ready");
  }

  fn start_metadata_edit(&mut self) {
    if self.view != ViewMode::Metadata {
      self.enter_metadata_view();
      return;
    }
    let draft = metadata::metadata_edit_draft(&self.document.path, &self.metadata);
    self.set_editor_request(EditorRequest::Metadata {
      original: self.metadata.clone(),
      draft,
    });
    self.set_message("editing metadata");
  }

  fn metadata_scroll_down(&mut self) {
    self.metadata_scroll = self.metadata_scroll.saturating_add(1);
  }

  fn metadata_scroll_up(&mut self) {
    self.metadata_scroll = self.metadata_scroll.saturating_sub(1);
  }

  fn metadata_page_down(&mut self) {
    self.metadata_scroll = self
      .metadata_scroll
      .saturating_add(self.viewport_height.max(1));
  }

  fn metadata_page_up(&mut self) {
    self.metadata_scroll = self
      .metadata_scroll
      .saturating_sub(self.viewport_height.max(1));
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
}

fn reload_document(
  path: PathBuf,
  cache_dir: PathBuf,
  render: config::RenderConfig,
) -> Result<DocumentReload, String> {
  let document =
    PdfDocument::open(path.clone(), cache_dir, &render).map_err(|error| error.to_string())?;
  let metadata = metadata::read_pdf_metadata(&path);
  let bookmarks = bookmarks::read_pdf_bookmarks(&path, &render.pdftk_bin, document.page_count);
  Ok(DocumentReload {
    document,
    metadata,
    bookmarks,
  })
}
