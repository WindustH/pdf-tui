//! The `:` command prompt: editing, completion, and running commands.

use crossterm::event::KeyEvent;
use framework_tui::{
  CommandCompletion, Prompt, PromptInputResult, current_word_start, filter_completion_candidates,
  handle_prompt_key as framework_handle_prompt_key,
  handle_prompt_paste as framework_handle_prompt_paste,
};
use tokio::sync::mpsc;

use crate::{config, event::AsyncEvent};

use super::App;

const COMMAND_NAMES: &[&str] = &[
  "clear-cache",
  "bookmarks",
  "help",
  "layout",
  "layout-use",
  "metadata",
  "quit",
  "refresh",
  "search",
  "selection",
  "write-config",
];

impl App {
  pub(super) fn start_command(&mut self) {
    self.command_state.reset_prompt_state();
    self.prompt = Some(Prompt::command(String::new()));
    self.refresh_command_completion();
  }

  pub(super) fn handle_prompt_key(
    &mut self,
    key: KeyEvent,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) {
    let result = if let Some(prompt) = self.prompt.as_mut() {
      framework_handle_prompt_key(prompt, &mut self.command_state, &self.keymap, key)
    } else {
      PromptInputResult::Unhandled
    };
    self.handle_prompt_input_result(result, Some(tx));
  }

  pub(super) fn handle_prompt_paste(&mut self, value: &str) {
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
      "search" => self.enter_search_view(tx),
      "selection" => self.enter_selection_view(),
      "refresh" => self.request_refresh(tx),
      "help" => self.show_key_help(),
      other => self.set_message(format!("unknown command: {other}")),
    }
  }

  pub(super) fn execute_layout_command(&mut self, command: &str, persist: bool) {
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
        // Drop the scroll layout built for the previous geometry: it is
        // stale now, and applying the preserved progress against its row
        // indices would land the new layout at an arbitrary position.
        // Clearing it routes the progress through `pending_progress`,
        // which is resolved once the new scroll layout is built.
        self.scroll_layout = None;
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
