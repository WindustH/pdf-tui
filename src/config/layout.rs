use std::collections::BTreeMap;

use framework_tui::layout::{Composition, ResolvedComposition};
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
  pub show_page_numbers: bool,
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
      show_page_numbers: false,
      padding: default_padding(),
      presets: default_layout_presets(),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LayoutPresetConfig {
  pub composition: Option<Composition<Box<LayoutPresetConfig>>>,
  #[serde(default = "default_layout_strategy")]
  pub strategy: String,
  pub params: Vec<String>,
  pub columns: u16,
  pub rows: u16,
  pub scroll_divisor: u16,
  pub gap_x: Option<u16>,
  pub gap_y: Option<u16>,
  pub show_border: Option<bool>,
  pub show_page_numbers: Option<bool>,
  pub padding: Option<u16>,
}

impl LayoutPresetConfig {
  fn slides() -> Self {
    Self {
      strategy: "composite".into(),
      composition: Some(slides_composition()),
      params: vec!["first_ratio".into(), "second_ratio".into()],
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
      composition: None,
      strategy: default_layout_strategy(),
      params: Vec::new(),
      columns: 0,
      rows: 0,
      scroll_divisor: 0,
      gap_x: None,
      gap_y: None,
      show_border: None,
      show_page_numbers: None,
      padding: None,
    }
  }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveLayoutConfig {
  pub composition: Option<Box<ResolvedComposition<EffectiveLayoutConfig, Box<LayoutPresetConfig>>>>,
  pub name: String,
  pub strategy: String,
  pub first_ratio: u16,
  pub second_ratio: u16,
  pub columns: u16,
  pub rows: u16,
  pub scroll_divisor: u16,
  pub gap_x: u16,
  pub gap_y: u16,
  pub show_border: bool,
  pub show_page_numbers: bool,
  pub padding: u16,
}

impl EffectiveLayoutConfig {
  pub fn label(&self) -> String {
    if let Some(composition) = &self.composition {
      return format!(
        "{} {}:{}",
        self.name, composition.split.ratio[0], composition.split.ratio[1]
      );
    }
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
    self.resolve(name, raw_args, &[])
  }

  fn resolve(
    &self,
    name: &str,
    raw_args: &[String],
    ancestors: &[String],
  ) -> Result<EffectiveLayoutConfig, String> {
    let preset = self
      .presets
      .get(name)
      .ok_or_else(|| format!("unknown layout: {name}"))?;
    self.resolve_panel(name, preset, raw_args, ancestors)
  }

  fn resolve_panel(
    &self,
    name: &str,
    preset: &LayoutPresetConfig,
    raw_args: &[String],
    ancestors: &[String],
  ) -> Result<EffectiveLayoutConfig, String> {
    if !ancestors.is_empty() && (preset.composition.is_some() || preset.strategy == "composite") {
      return Err("nested compositions are not supported".into());
    }
    if raw_args.len() > preset.params.len() {
      return Err(layout_usage(name, preset));
    }

    let mut effective = EffectiveLayoutConfig {
      composition: None,
      name: name.to_string(),
      strategy: normalize_layout_strategy(&preset.strategy),
      first_ratio: preset
        .composition
        .as_ref()
        .map_or(1, |split| split.ratio[0]),
      second_ratio: preset
        .composition
        .as_ref()
        .map_or(3, |split| split.ratio[1]),
      columns: preset.columns,
      rows: preset.rows,
      scroll_divisor: preset.scroll_divisor,
      gap_x: preset.gap_x.unwrap_or(self.gap_x),
      gap_y: preset.gap_y.unwrap_or(self.gap_y),
      show_border: preset.show_border.unwrap_or(self.show_border),
      show_page_numbers: preset.show_page_numbers.unwrap_or(self.show_page_numbers),
      padding: preset.padding.unwrap_or(self.padding),
    };

    for (param, value) in preset.params.iter().zip(raw_args) {
      apply_layout_param(&mut effective, param, value)
        .map_err(|err| format!("{err}; {}", layout_usage(name, preset)))?;
    }
    normalize_effective_layout(&mut effective);
    if !matches!(effective.strategy.as_str(), "scroll" | "grid" | "composite") {
      return Err(format!("unknown layout strategy: {}", effective.strategy));
    }
    if effective.strategy == "composite" {
      let mut split = preset
        .composition
        .clone()
        .ok_or_else(|| "composite layout requires a composition table".to_string())?;
      for (param, _) in preset.params.iter().zip(raw_args) {
        match param.trim().to_ascii_lowercase().as_str() {
          "first_ratio" => split.ratio[0] = effective.first_ratio,
          "second_ratio" => split.ratio[1] = effective.second_ratio,
          _ => {}
        }
      }
      split.validate()?;
      let mut ancestors = ancestors.to_vec();
      ancestors.push(name.into());
      let first = self.resolve_panel(&format!("{name}.first"), &split.first, &[], &ancestors)?;
      let second = self.resolve_panel(&format!("{name}.second"), &split.second, &[], &ancestors)?;
      if first.composition.is_some() || second.composition.is_some() {
        return Err("nested compositions are not supported".into());
      }
      let panels = [first, second];
      effective = panels[split.primary.index()].clone();
      effective.name = name.into();
      effective.composition = Some(Box::new(ResolvedComposition { split, panels }));
    }
    Ok(effective)
  }
}

impl LayoutPresetConfig {
  fn fill_missing_from(&mut self, default: &LayoutPresetConfig) {
    if self.strategy.is_empty() {
      self.strategy.clone_from(&default.strategy);
    }
    if self.strategy == default.strategy {
      if self.composition.is_none() {
        self.composition.clone_from(&default.composition);
      }
      if let (Some(split), Some(default_split)) = (&mut self.composition, &default.composition) {
        split.first.fill_missing_from(&default_split.first);
        split.second.fill_missing_from(&default_split.second);
      }
      if self.params.is_empty() {
        self.params.clone_from(&default.params);
      }
      if self.columns == 0 {
        self.columns = default.columns;
      }
      if self.rows == 0 {
        self.rows = default.rows;
      }
    }
    if self.show_page_numbers.is_none() {
      self.show_page_numbers = default.show_page_numbers;
    }
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
    composition: None,
    name: "scroll".to_string(),
    strategy: fallback.strategy,
    first_ratio: 1,
    second_ratio: 3,
    columns: fallback.columns,
    rows: fallback.rows,
    scroll_divisor: fallback.scroll_divisor,
    gap_x: config.gap_x,
    gap_y: config.gap_y,
    show_border: config.show_border,
    show_page_numbers: config.show_page_numbers,
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
    "" | "scroll" | "continuous" => "scroll".to_string(),
    "grid" | "fixed_grid" | "fixed-grid" => "grid".to_string(),
    other => other.to_string(),
  }
}

fn normalize_effective_layout(layout: &mut EffectiveLayoutConfig) {
  layout.strategy = normalize_layout_strategy(&layout.strategy);
  layout.first_ratio = layout.first_ratio.max(1);
  layout.second_ratio = layout.second_ratio.max(1);
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
    "first_ratio" => layout.first_ratio = parse_layout_ratio(param, value)?,
    "second_ratio" => layout.second_ratio = parse_layout_ratio(param, value)?,
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
    "show_page_numbers" => layout.show_page_numbers = parse_layout_bool(param, value)?,
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
  String::new()
}

fn default_layout_presets() -> BTreeMap<String, LayoutPresetConfig> {
  let mut presets = BTreeMap::new();
  presets.insert("slides".to_string(), LayoutPresetConfig::slides());

  presets.insert("scroll".to_string(), LayoutPresetConfig::scroll());
  presets.insert("grid".to_string(), LayoutPresetConfig::grid());
  presets
}

fn parse_layout_ratio(param: &str, value: &str) -> Result<u16, String> {
  let ratio = parse_layout_u16(param, value)?;
  if ratio == 0 {
    return Err(format!("{param} must be greater than zero"));
  }
  Ok(ratio)
}

fn slides_composition() -> Composition<Box<LayoutPresetConfig>> {
  Composition {
    first: Box::new(slides_sidebar()),
    second: Box::new(slides_main()),
    ..Composition::default()
  }
}

fn slides_sidebar() -> LayoutPresetConfig {
  LayoutPresetConfig {
    columns: 1,
    scroll_divisor: 16,
    show_page_numbers: Some(true),
    ..LayoutPresetConfig::scroll()
  }
}

fn slides_main() -> LayoutPresetConfig {
  LayoutPresetConfig {
    columns: 1,
    scroll_divisor: 1,
    show_page_numbers: Some(false),
    ..LayoutPresetConfig::scroll()
  }
}
