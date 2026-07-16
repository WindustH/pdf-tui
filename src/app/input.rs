use crossterm::event::{Event, KeyEvent, MouseEventKind};
use framework_tui::{
  CommandCompletion, KeyContext, MatchResult, Prompt, PromptInputResult, current_word_start,
  filter_completion_candidates, handle_prompt_key as framework_handle_prompt_key,
  handle_prompt_paste as framework_handle_prompt_paste, key_event_to_token,
};
use tokio::sync::mpsc;

use crate::{
  cache, config,
  event::{AsyncEvent, CacheClearOutcome},
};

use super::{App, CompletionRedrawState, InputRedrawState, PromptRedrawState};

const COMMAND_NAMES: &[&str] = &[
  "clear-cache",
  "help",
  "layout",
  "layout-use",
  "quit",
  "write-config",
];

impl App {
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
}
