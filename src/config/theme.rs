use ratatui::style::Color;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
  pub foreground: String,
  pub background: String,
  pub muted: String,
  pub accent: String,
  pub border: String,
  pub focused_border: String,
  pub selected_border: String,
  #[serde(default = "default_selected_foreground")]
  pub selected_foreground: String,
  pub selected_background: String,
  #[serde(default = "default_hover_foreground")]
  pub hover_foreground: String,
  #[serde(default = "default_hover_background")]
  pub hover_background: String,
  #[serde(default = "default_hover_selected_foreground")]
  pub hover_selected_foreground: String,
  #[serde(default = "default_hover_selected_background")]
  pub hover_selected_background: String,
  pub bookmark_hover_foreground: String,
  pub bookmark_hover_background: String,
  pub bookmark_page_color: String,
  pub bookmark_hover_page_color: String,
  pub bookmark_expanded_color: String,
  pub bookmark_collapsed_color: String,
  pub bookmark_leaf_color: String,
  pub error: String,
  pub which_key_columns: u16,
  pub which_key_background: String,
  pub which_key_foreground: String,
  pub which_key_key: String,
  pub which_key_rest: String,
  pub which_key_description: String,
  pub which_key_separator: String,
  pub which_key_separator_color: String,
}

impl Default for ThemeConfig {
  fn default() -> Self {
    Self {
      foreground: "white".to_string(),
      background: "reset".to_string(),
      muted: "dark_gray".to_string(),
      accent: "cyan".to_string(),
      border: "dark_gray".to_string(),
      focused_border: "yellow".to_string(),
      selected_border: "green".to_string(),
      selected_foreground: default_selected_foreground(),
      selected_background: "white".to_string(),
      hover_foreground: default_hover_foreground(),
      hover_background: default_hover_background(),
      hover_selected_foreground: default_hover_selected_foreground(),
      hover_selected_background: default_hover_selected_background(),
      bookmark_hover_foreground: "white".to_string(),
      bookmark_hover_background: "blue".to_string(),
      bookmark_page_color: "dark_gray".to_string(),
      bookmark_hover_page_color: "white".to_string(),
      bookmark_expanded_color: "white".to_string(),
      bookmark_collapsed_color: "yellow".to_string(),
      bookmark_leaf_color: "dark_gray".to_string(),
      error: "red".to_string(),
      which_key_columns: 3,
      which_key_background: "black".to_string(),
      which_key_foreground: "white".to_string(),
      which_key_key: "light_cyan".to_string(),
      which_key_rest: "dark_gray".to_string(),
      which_key_description: "light_magenta".to_string(),
      which_key_separator: " -> ".to_string(),
      which_key_separator_color: "dark_gray".to_string(),
    }
  }
}

impl ThemeConfig {
  pub fn color(&self, value: &str) -> Color {
    parse_color(value)
  }

  pub(super) fn normalize_defaults(&mut self) {}
}

fn default_selected_foreground() -> String {
  "auto".to_string()
}

fn default_hover_foreground() -> String {
  "auto".to_string()
}

fn default_hover_background() -> String {
  "cyan".to_string()
}

fn default_hover_selected_foreground() -> String {
  "auto".to_string()
}

fn default_hover_selected_background() -> String {
  "green".to_string()
}

fn parse_color(value: &str) -> Color {
  let lower = value.trim().to_ascii_lowercase();
  match lower.as_str() {
    "reset" => Color::Reset,
    "black" => Color::Black,
    "red" => Color::Red,
    "green" => Color::Green,
    "yellow" => Color::Yellow,
    "blue" => Color::Blue,
    "magenta" => Color::Magenta,
    "cyan" => Color::Cyan,
    "gray" | "grey" => Color::Gray,
    "dark_gray" | "dark_grey" | "darkgray" | "darkgrey" => Color::DarkGray,
    "light_red" | "lightred" => Color::LightRed,
    "light_green" | "lightgreen" => Color::LightGreen,
    "light_yellow" | "lightyellow" => Color::LightYellow,
    "light_blue" | "lightblue" => Color::LightBlue,
    "light_magenta" | "lightmagenta" => Color::LightMagenta,
    "light_cyan" | "lightcyan" => Color::LightCyan,
    "white" => Color::White,
    _ => {
      if let Some(raw) = lower.strip_prefix("ansi:") {
        return raw
          .parse::<u8>()
          .map(Color::Indexed)
          .unwrap_or(Color::Reset);
      }
      if lower.len() == 7 && lower.starts_with('#') {
        let r = u8::from_str_radix(&lower[1..3], 16);
        let g = u8::from_str_radix(&lower[3..5], 16);
        let b = u8::from_str_radix(&lower[5..7], 16);
        if let (Ok(r), Ok(g), Ok(b)) = (r, g, b) {
          return Color::Rgb(r, g, b);
        }
      }
      Color::Reset
    }
  }
}
