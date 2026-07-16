use std::fmt::Write as FmtWrite;

use framework_tui::{KeyBindingConfig, KeyBindings};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KeymapConfig {
  pub viewer: KeymapSection,
  pub metadata: KeymapSection,
  #[serde(default = "default_input_keymap_section")]
  pub input: KeymapSection,
  pub global: KeymapSection,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct KeymapSection {
  pub keymap: Vec<KeymapEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeymapEntry {
  pub on: KeymapOn,
  pub run: String,
  pub desc: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KeymapOn {
  One(String),
  Many(Vec<String>),
}

impl Default for KeymapConfig {
  fn default() -> Self {
    Self {
      viewer: KeymapSection {
        keymap: vec![
          key("q", "quit", "Quit pdf-tui"),
          key("ctrl-c", "quit", "Quit pdf-tui"),
          key("f1", "help", "Show viewer key bindings"),
          key("j", "scroll_down", "Scroll down"),
          key("down", "scroll_down", "Scroll down"),
          key("k", "scroll_up", "Scroll up"),
          key("up", "scroll_up", "Scroll up"),
          key("pgdn", "page_down", "Move one page down"),
          key("pagedown", "page_down", "Move one page down"),
          key("pgup", "page_up", "Move one page up"),
          key("home", "home", "Go to first page"),
          key(["g", "g"], "home", "Go to first page"),
          key("end", "end", "Go to last page"),
          key("G", "end", "Go to last page"),
          key("h", "page_up", "Move one page up"),
          key("left", "page_up", "Move one page up"),
          key("l", "page_down", "Move one page down"),
          key("right", "page_down", "Move one page down"),
          key("r", "refresh", "Refresh current PDF"),
          key("m", "metadata", "Show PDF metadata"),
          key(
            ["L", "s"],
            "layout scroll 1 3",
            "Use one-column scroll layout",
          ),
          key(["L", "g"], "layout grid 2 2", "Use 2x2 grid layout"),
        ],
      },
      metadata: KeymapSection {
        keymap: vec![
          key("q", "back", "Return to viewer"),
          key("esc", "back", "Return to viewer"),
          key("ctrl-c", "quit", "Quit pdf-tui"),
          key("f1", "help", "Show metadata key bindings"),
          key("e", "edit_metadata", "Edit PDF metadata"),
          key("j", "metadata_scroll_down", "Scroll metadata down"),
          key("down", "metadata_scroll_down", "Scroll metadata down"),
          key("k", "metadata_scroll_up", "Scroll metadata up"),
          key("up", "metadata_scroll_up", "Scroll metadata up"),
          key("pgdn", "metadata_page_down", "Scroll metadata page down"),
          key(
            "pagedown",
            "metadata_page_down",
            "Scroll metadata page down",
          ),
          key("pgup", "metadata_page_up", "Scroll metadata page up"),
        ],
      },
      input: default_input_keymap_section(),
      global: KeymapSection {
        keymap: vec![key(":", "command", "Enter command")],
      },
    }
  }
}

impl KeymapConfig {
  pub fn bindings(&self) -> KeyBindings {
    KeyBindings::from_sections(
      binding_configs(&self.viewer.keymap),
      binding_configs(&self.metadata.keymap),
      binding_configs(&self.input.keymap),
      binding_configs(&self.global.keymap),
    )
  }

  pub(super) fn normalize_defaults(&mut self) {
    let default = KeymapConfig::default();
    append_missing_actions(&mut self.viewer.keymap, &default.viewer.keymap);
    append_missing_actions(&mut self.metadata.keymap, &default.metadata.keymap);
    append_missing_actions(&mut self.input.keymap, &default.input.keymap);
    append_missing_actions(&mut self.global.keymap, &default.global.keymap);
  }
}

pub(super) fn format_keymap_toml(config: &KeymapConfig) -> String {
  let mut out = String::new();
  push_keymap_section(&mut out, "viewer", &config.viewer);
  push_keymap_section(&mut out, "metadata", &config.metadata);
  push_keymap_section(&mut out, "input", &config.input);
  push_keymap_section(&mut out, "global", &config.global);
  out
}

fn binding_configs(entries: &[KeymapEntry]) -> Vec<KeyBindingConfig> {
  entries
    .iter()
    .map(|entry| KeyBindingConfig {
      on: keymap_on_values(&entry.on),
      action: entry.run.clone(),
      desc: entry.desc.clone(),
    })
    .collect()
}

fn keymap_on_values(on: &KeymapOn) -> Vec<String> {
  match on {
    KeymapOn::One(value) => vec![value.clone()],
    KeymapOn::Many(values) => values.clone(),
  }
}

fn append_missing_actions(entries: &mut Vec<KeymapEntry>, defaults: &[KeymapEntry]) {
  for default in defaults {
    if entries.iter().any(|entry| entry.run == default.run) {
      continue;
    }
    entries.push(default.clone());
  }
}

fn default_input_keymap_section() -> KeymapSection {
  KeymapSection {
    keymap: vec![
      key("esc", "cancel", "Cancel input"),
      key("f1", "help", "Show input key bindings"),
      key("enter", "submit", "Submit input"),
      key("backspace", "backspace", "Delete before cursor"),
      key("delete", "delete", "Delete under cursor"),
      key("left", "move_left", "Move cursor left"),
      key("right", "move_right", "Move cursor right"),
      key("home", "move_start", "Move cursor to start"),
      key("ctrl-a", "move_start", "Move cursor to start"),
      key("end", "move_end", "Move cursor to end"),
      key("ctrl-e", "move_end", "Move cursor to end"),
      key("ctrl-u", "kill_before_cursor", "Delete before cursor"),
      key("ctrl-k", "kill_after_cursor", "Delete after cursor"),
      key("tab", "completion_next", "Select next completion"),
      key(
        "backtab",
        "completion_previous",
        "Select previous completion",
      ),
      key("up", "history_previous", "Previous command history"),
      key("down", "history_next", "Next command history"),
    ],
  }
}

fn key(on: impl Into<KeymapOn>, run: &str, desc: &str) -> KeymapEntry {
  KeymapEntry {
    on: on.into(),
    run: run.to_string(),
    desc: desc.to_string(),
  }
}

impl From<&str> for KeymapOn {
  fn from(value: &str) -> Self {
    Self::One(value.to_string())
  }
}

impl<const N: usize> From<[&str; N]> for KeymapOn {
  fn from(value: [&str; N]) -> Self {
    Self::Many(value.into_iter().map(str::to_string).collect())
  }
}

fn push_keymap_section(out: &mut String, name: &str, section: &KeymapSection) {
  let _ = writeln!(out, "[{name}]");
  out.push_str("keymap = [\n");
  for entry in &section.keymap {
    let _ = writeln!(
      out,
      "  {{ on = {}, run = {}, desc = {} }},",
      format_keymap_on(&entry.on),
      toml_basic_string(&entry.run),
      toml_basic_string(&entry.desc)
    );
  }
  out.push_str("]\n\n");
}

fn format_keymap_on(on: &KeymapOn) -> String {
  match on {
    KeymapOn::One(value) => toml_basic_string(value),
    KeymapOn::Many(values) => {
      let keys = values
        .iter()
        .map(|value| toml_basic_string(value))
        .collect::<Vec<_>>()
        .join(", ");
      format!("[{keys}]")
    }
  }
}

fn toml_basic_string(value: &str) -> String {
  let mut out = String::with_capacity(value.len() + 2);
  out.push('"');
  for ch in value.chars() {
    match ch {
      '\\' => out.push_str("\\\\"),
      '"' => out.push_str("\\\""),
      '\n' => out.push_str("\\n"),
      '\r' => out.push_str("\\r"),
      '\t' => out.push_str("\\t"),
      '\u{08}' => out.push_str("\\b"),
      '\u{0c}' => out.push_str("\\f"),
      ch if ch.is_control() => {
        let _ = write!(out, "\\u{:04X}", ch as u32);
      }
      ch => out.push(ch),
    }
  }
  out.push('"');
  out
}
