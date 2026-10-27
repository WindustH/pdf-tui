use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Upper bound for page rows, page columns, and the scroll divisor. Larger
/// values cannot fit a terminal anyway, and a grid allocates one slot per
/// rows x columns cell, so unbounded values could exhaust memory.
const MAX_LAYOUT_COUNT: u16 = 64;

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
  pub left_ratio: u16,
  pub right_ratio: u16,
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
  fn slides() -> Self {
    Self {
      strategy: "slides".to_string(),
      params: vec!["left_ratio".to_string(), "right_ratio".to_string()],
      columns: 1,
      rows: 1,
      show_border: Some(false),
      padding: Some(0),
      ..Self::default()
    }
  }

  fn scroll() -> Self {
    Self {
      strategy: "scroll".to_string(),
      params: vec!["columns".to_string(), "scroll_divisor".to_string()],
      columns: 1,
      rows: 1,
      scroll_divisor: 1,
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
      scroll_divisor: 1,
      show_border: Some(false),
      padding: Some(0),
      ..Self::default()
    }
  }
}

impl Default for LayoutPresetConfig {
  fn default() -> Self {
    Self {
      strategy: default_layout_strategy(),
      left_ratio: 1,
      right_ratio: 3,
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
  pub left_ratio: u16,
  pub right_ratio: u16,
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
      "slides" => format!("{} {}:{}", self.name, self.left_ratio, self.right_ratio),
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
  pub(super) fn normalize_defaults(&mut self) {
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

  pub(super) fn validate(&self) -> Result<(), String> {
    self.effective_for(&self.active, &self.active_args)?;
    Ok(())
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
      left_ratio: preset.left_ratio,
      right_ratio: preset.right_ratio,
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
    left_ratio: fallback.left_ratio,
    right_ratio: fallback.right_ratio,
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
  layout.left_ratio = layout.left_ratio.max(1);
  layout.right_ratio = layout.right_ratio.max(1);
  if layout.strategy == "slides" {
    layout.columns = 1;
    layout.rows = 1;
  }
  layout.columns = layout.columns.clamp(1, MAX_LAYOUT_COUNT);
  layout.rows = layout.rows.clamp(1, MAX_LAYOUT_COUNT);
  layout.scroll_divisor = layout.scroll_divisor.clamp(1, MAX_LAYOUT_COUNT);
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
    "left_ratio" | "left-ratio" => layout.left_ratio = parse_layout_ratio(param, value)?,
    "right_ratio" | "right-ratio" => layout.right_ratio = parse_layout_ratio(param, value)?,
    "columns" | "column" | "cols" => layout.columns = parse_layout_count(param, value)?,
    "rows" | "row" => layout.rows = parse_layout_count(param, value)?,
    "scroll_divisor" | "scroll-divisor" | "divisor" | "step" | "chunk" => {
      layout.scroll_divisor = parse_layout_count(param, value)?
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

fn parse_layout_count(param: &str, value: &str) -> Result<u16, String> {
  let parsed = parse_layout_u16(param, value)?;
  if parsed > MAX_LAYOUT_COUNT {
    return Err(format!("{param} must be at most {MAX_LAYOUT_COUNT}"));
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
  vec!["2".to_string(), "3".to_string()]
}

fn default_gap_x() -> u16 {
  2
}

fn default_gap_y() -> u16 {
  1
}

fn default_show_border() -> bool {
  false
}

fn default_padding() -> u16 {
  0
}

fn default_layout_strategy() -> String {
  "scroll".to_string()
}

fn default_layout_presets() -> BTreeMap<String, LayoutPresetConfig> {
  let mut presets = BTreeMap::new();
  presets.insert("slides".to_string(), LayoutPresetConfig::slides());
  presets.insert("scroll".to_string(), LayoutPresetConfig::scroll());
  presets.insert("grid".to_string(), LayoutPresetConfig::grid());
  presets
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn layout_counts_are_bounded() {
    let mut config = LayoutConfig::default();
    let error = config
      .set_active_from_args("grid", &["65535", "65535"])
      .unwrap_err();
    assert!(error.contains("at most"), "{error}");
    let layout = config.set_active_from_args("grid", &["3", "4"]).unwrap();
    assert_eq!((layout.rows, layout.columns), (3, 4));

    // Oversized values from config.toml are clamped rather than rejected.
    let preset = config.presets.get_mut("grid").unwrap();
    preset.params.clear();
    preset.rows = 1000;
    preset.columns = 0;
    let layout = config.set_active_from_args("grid", &[]).unwrap();
    assert_eq!((layout.rows, layout.columns), (MAX_LAYOUT_COUNT, 1));
  }
}

fn parse_layout_ratio(param: &str, value: &str) -> Result<u16, String> {
  let ratio = parse_layout_u16(param, value)?;
  if ratio == 0 {
    return Err(format!("{param} must be greater than zero"));
  }
  Ok(ratio)
}

#[cfg(test)]
mod slides_tests {
  use super::*;

  #[test]
  fn slides_ratios_and_invalid_arguments() {
    let mut config = LayoutConfig::default();
    let defaults = config.set_active_from_args("slides", &[]).unwrap();
    assert_eq!((defaults.left_ratio, defaults.right_ratio), (1, 3));
    assert!(!defaults.show_border);
    let custom = config.set_active_from_args("slides", &["2", "5"]).unwrap();
    assert_eq!((custom.left_ratio, custom.right_ratio), (2, 5));
    assert_eq!((custom.columns, custom.rows), (1, 1));
    for args in [
      vec!["0", "3"],
      vec!["1", "0"],
      vec!["-1"],
      vec!["1.5"],
      vec!["65536"],
      vec!["1", "2", "3"],
    ] {
      assert!(config.set_active_from_args("slides", &args).is_err());
      assert_eq!(config.effective(), custom);
    }
    let saved = toml::to_string(&config).unwrap();
    let restored: LayoutConfig = toml::from_str(&saved).unwrap();
    assert_eq!(restored.effective(), custom);
  }
}
