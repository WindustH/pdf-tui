use std::{
  collections::BTreeMap,
  env,
  fmt::Write as FmtWrite,
  path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use framework_tui::{KeyBindingConfig, KeyBindings};
use img_tui::TerminalCapability;
use ratatui::style::Color;
use serde::{Deserialize, Serialize};
use tokio::fs;

#[derive(Debug, Clone)]
pub struct Settings {
  pub config: AppConfig,
  pub keymap: KeymapConfig,
  pub theme: ThemeConfig,
  pub config_path: PathBuf,
  pub cache_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
  pub layout: LayoutConfig,
  pub render: RenderConfig,
  pub behavior: BehaviorConfig,
}

impl Default for AppConfig {
  fn default() -> Self {
    Self {
      layout: LayoutConfig::default(),
      render: RenderConfig::default(),
      behavior: BehaviorConfig::default(),
    }
  }
}

impl AppConfig {
  fn normalize_defaults(&mut self) {
    self.layout.normalize_defaults();
    self.render.normalize_defaults();
    self.behavior.normalize_defaults();
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutConfig {
  #[serde(default = "default_layout_active")]
  pub active: String,
  #[serde(default = "default_layout_active_args")]
  #[serde(skip_serializing_if = "Vec::is_empty")]
  pub active_args: Vec<String>,
  #[serde(default = "default_gap_x")]
  pub gap_x: u16,
  #[serde(default = "default_gap_y")]
  pub gap_y: u16,
  #[serde(default = "default_show_border")]
  pub show_border: bool,
  #[serde(default = "default_padding")]
  pub padding: u16,
  #[serde(default = "default_layout_presets")]
  pub presets: BTreeMap<String, LayoutPresetConfig>,
}

impl Default for LayoutConfig {
  fn default() -> Self {
    Self {
      active: default_layout_active(),
      active_args: default_layout_active_args(),
      gap_x: default_gap_x(),
      gap_y: default_gap_y(),
      show_border: default_show_border(),
      padding: default_padding(),
      presets: default_layout_presets(),
    }
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutPresetConfig {
  #[serde(default = "default_layout_strategy")]
  pub strategy: String,
  pub params: Vec<String>,
  pub columns: u16,
  pub rows: u16,
  pub scroll_divisor: u16,
  pub gap_x: Option<u16>,
  pub gap_y: Option<u16>,
  pub show_border: Option<bool>,
  pub padding: Option<u16>,
}

impl LayoutPresetConfig {
  fn scroll() -> Self {
    Self {
      strategy: "scroll".to_string(),
      params: vec!["columns".to_string(), "scroll_divisor".to_string()],
      columns: 1,
      rows: 1,
      scroll_divisor: 3,
      show_border: Some(false),
      padding: Some(0),
      ..Self::default()
    }
  }

  fn grid() -> Self {
    Self {
      strategy: "grid".to_string(),
      params: vec!["rows".to_string(), "columns".to_string()],
      columns: 2,
      rows: 2,
      scroll_divisor: 3,
      show_border: Some(true),
      padding: Some(1),
      ..Self::default()
    }
  }
}

impl Default for LayoutPresetConfig {
  fn default() -> Self {
    Self {
      strategy: default_layout_strategy(),
      params: Vec::new(),
      columns: 0,
      rows: 0,
      scroll_divisor: 0,
      gap_x: None,
      gap_y: None,
      show_border: None,
      padding: None,
    }
  }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveLayoutConfig {
  pub name: String,
  pub strategy: String,
  pub columns: u16,
  pub rows: u16,
  pub scroll_divisor: u16,
  pub gap_x: u16,
  pub gap_y: u16,
  pub show_border: bool,
  pub padding: u16,
}

impl EffectiveLayoutConfig {
  pub fn label(&self) -> String {
    match self.strategy.as_str() {
      "scroll" => format!(
        "{} {} {}",
        self.name,
        self.columns.max(1),
        self.scroll_divisor.max(1)
      ),
      "grid" => format!("{} {}x{}", self.name, self.rows.max(1), self.columns.max(1)),
      _ => self.name.clone(),
    }
  }

  pub fn is_scroll(&self) -> bool {
    self.strategy == "scroll"
  }

  pub fn grid_capacity(&self) -> usize {
    usize::from(self.rows.max(1)) * usize::from(self.columns.max(1))
  }
}

impl LayoutConfig {
  fn normalize_defaults(&mut self) {
    for (name, default_preset) in default_layout_presets() {
      match self.presets.get_mut(&name) {
        Some(preset) => preset.fill_missing_from(&default_preset),
        None => {
          self.presets.insert(name, default_preset);
        }
      }
    }
  }

  pub fn effective(&self) -> EffectiveLayoutConfig {
    self
      .effective_for(&self.active, &self.active_args)
      .unwrap_or_else(|_| default_effective_layout(self))
  }

  pub fn set_active_from_args(
    &mut self,
    name: &str,
    raw_args: &[&str],
  ) -> Result<EffectiveLayoutConfig, String> {
    let preset = self
      .presets
      .get(name)
      .ok_or_else(|| format!("unknown layout: {name}"))?;
    let args = normalize_layout_args(preset, raw_args)?;
    let effective = self.effective_for(name, &args)?;
    self.active = name.to_string();
    self.active_args = args;
    Ok(effective)
  }

  fn effective_for(
    &self,
    name: &str,
    raw_args: &[String],
  ) -> Result<EffectiveLayoutConfig, String> {
    let preset = self
      .presets
      .get(name)
      .ok_or_else(|| format!("unknown layout: {name}"))?;
    if raw_args.len() > preset.params.len() {
      return Err(layout_usage(name, preset));
    }

    let mut effective = EffectiveLayoutConfig {
      name: name.to_string(),
      strategy: normalize_layout_strategy(&preset.strategy),
      columns: preset.columns,
      rows: preset.rows,
      scroll_divisor: preset.scroll_divisor,
      gap_x: preset.gap_x.unwrap_or(self.gap_x),
      gap_y: preset.gap_y.unwrap_or(self.gap_y),
      show_border: preset.show_border.unwrap_or(self.show_border),
      padding: preset.padding.unwrap_or(self.padding),
    };

    for (param, value) in preset.params.iter().zip(raw_args) {
      apply_layout_param(&mut effective, param, value)
        .map_err(|err| format!("{err}; {}", layout_usage(name, preset)))?;
    }
    normalize_effective_layout(&mut effective);
    Ok(effective)
  }
}

impl LayoutPresetConfig {
  fn fill_missing_from(&mut self, default: &LayoutPresetConfig) {
    if self.scroll_divisor == 0 {
      self.scroll_divisor = default.scroll_divisor;
    }
    if self.gap_x.is_none() {
      self.gap_x = default.gap_x;
    }
    if self.gap_y.is_none() {
      self.gap_y = default.gap_y;
    }
    if self.show_border.is_none() {
      self.show_border = default.show_border;
    }
    if self.padding.is_none() {
      self.padding = default.padding;
    }
  }
}

fn default_effective_layout(config: &LayoutConfig) -> EffectiveLayoutConfig {
  let fallback = LayoutPresetConfig::scroll();
  let mut effective = EffectiveLayoutConfig {
    name: "scroll".to_string(),
    strategy: fallback.strategy,
    columns: fallback.columns,
    rows: fallback.rows,
    scroll_divisor: fallback.scroll_divisor,
    gap_x: config.gap_x,
    gap_y: config.gap_y,
    show_border: config.show_border,
    padding: config.padding,
  };
  normalize_effective_layout(&mut effective);
  effective
}

fn normalize_layout_args(
  preset: &LayoutPresetConfig,
  raw_args: &[&str],
) -> Result<Vec<String>, String> {
  let mut args = raw_args
    .iter()
    .map(|arg| arg.trim().to_string())
    .filter(|arg| !arg.is_empty())
    .collect::<Vec<_>>();

  if args.len() == 1
    && preset.params.len() >= 2
    && is_param(&preset.params[0], &["rows", "row"])
    && is_param(&preset.params[1], &["columns", "column", "cols"])
    && let Some((rows, columns)) = split_grid_shape(&args[0])
  {
    args = vec![rows, columns];
  }

  if args.len() > preset.params.len() {
    return Err("too many layout arguments".to_string());
  }
  Ok(args)
}

fn split_grid_shape(value: &str) -> Option<(String, String)> {
  let (rows, columns) = value.split_once('x').or_else(|| value.split_once('X'))?;
  let rows = rows.trim();
  let columns = columns.trim();
  if rows.is_empty() || columns.is_empty() {
    return None;
  }
  Some((rows.to_string(), columns.to_string()))
}

fn normalize_layout_strategy(strategy: &str) -> String {
  match strategy.trim().to_ascii_lowercase().as_str() {
    "scroll" | "continuous" => "scroll".to_string(),
    "grid" | "fixed_grid" | "fixed-grid" => "grid".to_string(),
    other => other.to_string(),
  }
}

fn normalize_effective_layout(layout: &mut EffectiveLayoutConfig) {
  layout.strategy = normalize_layout_strategy(&layout.strategy);
  layout.columns = layout.columns.max(1);
  layout.rows = layout.rows.max(1);
  layout.scroll_divisor = layout.scroll_divisor.max(1);
  if layout.strategy == "scroll" {
    layout.rows = 1;
  }
}

fn apply_layout_param(
  layout: &mut EffectiveLayoutConfig,
  param: &str,
  value: &str,
) -> Result<(), String> {
  match param.trim().to_ascii_lowercase().as_str() {
    "columns" | "column" | "cols" => layout.columns = parse_layout_u16(param, value)?,
    "rows" | "row" => layout.rows = parse_layout_u16(param, value)?,
    "scroll_divisor" | "scroll-divisor" | "divisor" | "step" | "chunk" => {
      layout.scroll_divisor = parse_layout_u16(param, value)?
    }
    "gap_x" | "gap-x" => layout.gap_x = parse_layout_u16(param, value)?,
    "gap_y" | "gap-y" => layout.gap_y = parse_layout_u16(param, value)?,
    "show_border" | "show-border" | "border" | "borders" => {
      layout.show_border = parse_layout_bool(param, value)?
    }
    "padding" | "pad" => layout.padding = parse_layout_u16(param, value)?,
    _ => return Err(format!("unknown layout parameter: {param}")),
  }
  Ok(())
}

fn parse_layout_u16(param: &str, value: &str) -> Result<u16, String> {
  let parsed = value
    .parse::<u16>()
    .map_err(|_| format!("{param} must be a non-negative integer"))?;
  if parsed == 0 {
    return Err(format!("{param} must be greater than zero"));
  }
  Ok(parsed)
}

fn parse_layout_bool(param: &str, value: &str) -> Result<bool, String> {
  match value.trim().to_ascii_lowercase().as_str() {
    "true" | "yes" | "on" | "1" => Ok(true),
    "false" | "no" | "off" | "0" => Ok(false),
    _ => Err(format!("{param} must be true or false")),
  }
}

fn is_param(value: &str, aliases: &[&str]) -> bool {
  let value = value.trim().to_ascii_lowercase();
  aliases.iter().any(|alias| value == *alias)
}

fn layout_usage(name: &str, preset: &LayoutPresetConfig) -> String {
  if preset.params.is_empty() {
    format!("usage: :layout {name}")
  } else {
    let params = preset
      .params
      .iter()
      .map(|param| format!("<{param}>"))
      .collect::<Vec<_>>()
      .join(" ");
    format!("usage: :layout {name} {params}")
  }
}

fn default_layout_active() -> String {
  "scroll".to_string()
}

fn default_layout_active_args() -> Vec<String> {
  vec!["1".to_string(), "3".to_string()]
}

fn default_gap_x() -> u16 {
  2
}

fn default_gap_y() -> u16 {
  1
}

fn default_show_border() -> bool {
  true
}

fn default_padding() -> u16 {
  1
}

fn default_layout_strategy() -> String {
  "scroll".to_string()
}

fn default_layout_presets() -> BTreeMap<String, LayoutPresetConfig> {
  let mut presets = BTreeMap::new();
  presets.insert("scroll".to_string(), LayoutPresetConfig::scroll());
  presets.insert("grid".to_string(), LayoutPresetConfig::grid());
  presets
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderConfig {
  pub pdfinfo_bin: String,
  pub pdftoppm_bin: String,
  pub page_dpi: u16,
  pub chafa_bin: String,
  pub auto_detect: bool,
  pub chafa_args: Vec<String>,
  pub cache_max_bytes: u64,
  pub cache_compression_level: i32,
  pub cache_compression_threads: u32,
  pub max_concurrent: usize,
  pub chafa_threads: usize,
  pub preload_ahead: usize,
  pub preload_behind: usize,
  pub passthrough: Option<String>,
  pub zellij_sixel: String,
}

impl Default for RenderConfig {
  fn default() -> Self {
    Self {
      pdfinfo_bin: "pdfinfo".to_string(),
      pdftoppm_bin: "pdftoppm".to_string(),
      page_dpi: 180,
      chafa_bin: "chafa".to_string(),
      auto_detect: true,
      chafa_args: vec![
        "--format=symbols".to_string(),
        "--colors=full".to_string(),
        "--symbols=block".to_string(),
        "--animate=off".to_string(),
        "--polite=on".to_string(),
      ],
      cache_max_bytes: 512 * 1024 * 1024,
      cache_compression_level: 3,
      cache_compression_threads: 2,
      max_concurrent: 4,
      chafa_threads: 1,
      preload_ahead: 4,
      preload_behind: 2,
      passthrough: None,
      zellij_sixel: "off".to_string(),
    }
  }
}

impl RenderConfig {
  pub fn apply_terminal_capability(&mut self, capability: &TerminalCapability) {
    self.chafa_args.retain(|arg| {
      !arg.starts_with("--format=")
        && !arg.starts_with("--colors=")
        && !arg.starts_with("--symbols=")
        && !arg.starts_with("--passthrough=")
    });
    self
      .chafa_args
      .insert(0, capability.symbols_arg().to_string());
    self
      .chafa_args
      .insert(0, capability.colors_arg().to_string());
    self.passthrough = capability.passthrough().map(str::to_string);
  }

  fn normalize_defaults(&mut self) {
    self.page_dpi = self.page_dpi.max(36);
    self.max_concurrent = self.max_concurrent.max(1);
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BehaviorConfig {
  pub scroll_lines: u16,
}

impl Default for BehaviorConfig {
  fn default() -> Self {
    Self { scroll_lines: 4 }
  }
}

impl BehaviorConfig {
  fn normalize_defaults(&mut self) {
    self.scroll_lines = self.scroll_lines.max(1);
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KeymapConfig {
  pub browser: KeymapSection,
  pub detail: KeymapSection,
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
      browser: KeymapSection {
        keymap: vec![
          key("q", "quit", "Quit pdf-tui"),
          key("ctrl-c", "quit", "Quit pdf-tui"),
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
          key("h", "previous_page", "Previous page"),
          key("left", "previous_page", "Previous page"),
          key("l", "next_page", "Next page"),
          key("right", "next_page", "Next page"),
          key(
            ["L", "s"],
            "layout scroll 1 3",
            "Use one-column scroll layout",
          ),
          key(["L", "g"], "layout grid 2 2", "Use 2x2 grid layout"),
        ],
      },
      detail: KeymapSection::default(),
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
      binding_configs(&self.browser.keymap),
      binding_configs(&self.detail.keymap),
      binding_configs(&self.input.keymap),
      binding_configs(&self.global.keymap),
    )
  }

  fn normalize_defaults(&mut self) {
    let default = KeymapConfig::default();
    append_missing_actions(&mut self.browser.keymap, &default.browser.keymap);
    append_missing_actions(&mut self.detail.keymap, &default.detail.keymap);
    append_missing_actions(&mut self.input.keymap, &default.input.keymap);
    append_missing_actions(&mut self.global.keymap, &default.global.keymap);
  }
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

  fn normalize_defaults(&mut self) {}
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

pub async fn load_or_create() -> Result<Settings> {
  let config_dir = app_config_dir();
  let cache_dir = app_cache_dir();

  fs::create_dir_all(&config_dir)
    .await
    .with_context(|| format!("failed to create {}", config_dir.display()))?;
  fs::create_dir_all(&cache_dir)
    .await
    .with_context(|| format!("failed to create {}", cache_dir.display()))?;

  let config_path = config_dir.join("config.toml");
  let config = read_or_write_default(&config_path, AppConfig::default()).await?;
  let keymap =
    read_or_write_keymap_default(&config_dir.join("keymap.toml"), KeymapConfig::default()).await?;
  let theme = read_or_write_default(&config_dir.join("theme.toml"), ThemeConfig::default()).await?;

  Ok(Settings {
    config,
    keymap,
    theme,
    config_path,
    cache_dir,
  })
}

fn app_config_dir() -> PathBuf {
  platform_config_dir().join("pdf-tui")
}

fn app_cache_dir() -> PathBuf {
  platform_cache_dir().join("pdf-tui")
}

fn platform_config_dir() -> PathBuf {
  env_path("XDG_CONFIG_HOME")
    .or_else(|| env_path("HOME").map(|home| home.join(".config")))
    .unwrap_or_else(|| PathBuf::from(".config"))
}

fn platform_cache_dir() -> PathBuf {
  env_path("XDG_CACHE_HOME")
    .or_else(|| env_path("HOME").map(|home| home.join(".cache")))
    .unwrap_or_else(|| PathBuf::from(".cache"))
}

fn env_path(name: &str) -> Option<PathBuf> {
  env::var_os(name)
    .filter(|value| !value.is_empty())
    .map(PathBuf::from)
}

pub fn write_app_config_sync(path: &Path, config: &AppConfig) -> Result<()> {
  let body = app_config_toml(config)?;
  std::fs::write(path, body).with_context(|| format!("failed to write {}", path.display()))
}

fn app_config_toml(config: &AppConfig) -> Result<String> {
  toml::to_string_pretty(config).map_err(Into::into)
}

async fn read_or_write_keymap_default(path: &Path, default: KeymapConfig) -> Result<KeymapConfig> {
  if !path.exists() {
    fs::write(path, format_keymap_toml(&default))
      .await
      .with_context(|| format!("failed to write {}", path.display()))?;
    return Ok(default);
  }
  let body = fs::read_to_string(path)
    .await
    .with_context(|| format!("failed to read {}", path.display()))?;
  let mut parsed: KeymapConfig =
    toml::from_str(&body).with_context(|| format!("failed to parse {}", path.display()))?;
  parsed.normalize_defaults();
  let normalized = format_keymap_toml(&parsed);
  write_back_if_toml_changed(path, &body, &normalized).await?;
  Ok(parsed)
}

async fn read_or_write_default<T>(path: &Path, default: T) -> Result<T>
where
  T: Serialize + for<'de> Deserialize<'de> + Clone + NormalizeConfigDefaults,
{
  if !path.exists() {
    let body = toml::to_string_pretty(&default)?;
    fs::write(path, body)
      .await
      .with_context(|| format!("failed to write {}", path.display()))?;
    let mut default = default;
    default.normalize_defaults();
    return Ok(default);
  }
  let body = fs::read_to_string(path)
    .await
    .with_context(|| format!("failed to read {}", path.display()))?;
  let mut parsed: T =
    toml::from_str(&body).with_context(|| format!("failed to parse {}", path.display()))?;
  parsed.normalize_defaults();
  let normalized = toml::to_string_pretty(&parsed)?;
  write_back_if_toml_changed(path, &body, &normalized).await?;
  Ok(parsed)
}

trait NormalizeConfigDefaults {
  fn normalize_defaults(&mut self);
}

impl NormalizeConfigDefaults for AppConfig {
  fn normalize_defaults(&mut self) {
    AppConfig::normalize_defaults(self);
  }
}

impl NormalizeConfigDefaults for ThemeConfig {
  fn normalize_defaults(&mut self) {
    ThemeConfig::normalize_defaults(self);
  }
}

async fn write_back_if_toml_changed(path: &Path, original: &str, normalized: &str) -> Result<()> {
  if toml_semantic_value(original) != toml_semantic_value(normalized) {
    fs::write(path, normalized)
      .await
      .with_context(|| format!("failed to update {}", path.display()))?;
  }
  Ok(())
}

fn toml_semantic_value(body: &str) -> Option<toml::Value> {
  toml::from_str(body).ok()
}

fn format_keymap_toml(config: &KeymapConfig) -> String {
  let mut out = String::new();
  push_keymap_section(&mut out, "browser", &config.browser);
  push_keymap_section(&mut out, "detail", &config.detail);
  push_keymap_section(&mut out, "input", &config.input);
  push_keymap_section(&mut out, "global", &config.global);
  out
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
