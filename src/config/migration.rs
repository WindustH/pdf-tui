//! One-way migrations of persisted configuration, before schema deserialization.
//! Runtime layout resolution accepts only the current composition schema.

use super::AppConfig;
use std::{
  io::ErrorKind,
  path::Path,
  time::{SystemTime, UNIX_EPOCH},
};
use tokio::{fs, io::AsyncWriteExt};

use anyhow::{Context, Result};
use framework_tui::layout::{Composition, LayoutReference};
use toml::Value;

/// Upgrade an existing app configuration. Conversion, validation, backup and
/// atomic replacement all live here; invalid migrations leave the source intact.
/// Malformed TOML is handled by the loader's normal invalid-file recovery.
pub(super) async fn run(path: &Path) -> Result<()> {
  let _lock = crate::cache::acquire_cache_file_lock(path).await?;
  let original = match fs::read_to_string(path).await {
    Ok(body) => body,
    Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
    Err(error) => return Err(error).with_context(|| format!("failed to read {}", path.display())),
  };
  let Ok(mut value) = toml::from_str::<Value>(&original) else {
    return Ok(());
  };
  let mut changed = migrate(&mut value)?;
  changed |= inline_panels(&mut value)?;
  if !changed {
    return Ok(());
  }
  let mut config: AppConfig = value
    .try_into()
    .context("invalid configuration after migration")?;
  config.normalize_defaults();
  config.layout.validate().map_err(anyhow::Error::msg)?;
  let mut validation = config.layout.clone();
  for name in config.layout.presets.keys() {
    validation
      .set_active_from_args(name, &[])
      .map_err(anyhow::Error::msg)?;
  }
  let normalized = super::app_config_toml(&config)?;
  backup(path, &original).await?;
  crate::cache::write_bytes_atomic(path, normalized.as_bytes())
    .await
    .with_context(|| format!("failed to save migrated configuration {}", path.display()))
}

/// Copy rather than rename: the original remains available until replacement
/// succeeds. Exclusive creation prevents concurrent migrations overwriting backups.
async fn backup(path: &Path, original: &str) -> Result<()> {
  let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
  let mut name = path
    .file_name()
    .context("configuration path needs a filename")?
    .to_os_string();
  name.push(format!(".bak.{}.{stamp}", std::process::id()));
  let backup = path.with_file_name(name);
  let mut file = fs::OpenOptions::new()
    .write(true)
    .create_new(true)
    .open(&backup)
    .await
    .with_context(|| format!("failed to create migration backup {}", backup.display()))?;
  file.write_all(original.as_bytes()).await?;
  file.sync_all().await?;
  Ok(())
}

/// Convert legacy slides presets and remove obsolete panel-ratio fields.
/// Existing compositions take precedence over legacy fields. Positional args
/// keep their order, so active_args, keymaps and panel references stay valid.
fn migrate(value: &mut Value) -> Result<bool> {
  let Some(presets) = value
    .get_mut("layout")
    .and_then(|layout| layout.get_mut("presets"))
    .and_then(Value::as_table_mut)
  else {
    return Ok(false);
  };
  let mut changed = false;
  for (name, value) in presets {
    let preset = value
      .as_table_mut()
      .context("layout preset must be a table")?;
    let legacy_slides = preset
      .get("strategy")
      .and_then(Value::as_str)
      .map_or(name == "slides", |strategy| {
        strategy.trim().eq_ignore_ascii_case("slides")
      });
    if legacy_slides {
      if !preset.contains_key("composition") {
        let split = Composition {
          ratio: [
            ratio(preset.get("left_ratio"), 1)?,
            ratio(preset.get("right_ratio"), 3)?,
          ],
          gap: match preset.get("gap_x") {
            Some(value) => u16::try_from(value.as_integer().context("gap_x must be an integer")?)
              .context("gap_x must fit in u16")?
              .max(1),
            None => 1,
          },
          first: LayoutReference::new("slides-sidebar", &[]),
          second: LayoutReference::new("slides-main", &[]),
          ..Composition::default()
        };
        preset.insert("composition".into(), Value::try_from(split)?);
      }
      preset.insert("strategy".into(), Value::String("composite".into()));
      changed = true;
    }
    if let Some(params) = preset.get_mut("params").and_then(Value::as_array_mut) {
      for param in params {
        let replacement = match param
          .as_str()
          .map(|s| s.trim().to_ascii_lowercase())
          .as_deref()
        {
          Some("left_ratio" | "left-ratio") => Some("first_ratio"),
          Some("right_ratio" | "right-ratio") => Some("second_ratio"),
          _ => None,
        };
        if let Some(replacement) = replacement {
          *param = Value::String(replacement.into());
          changed = true;
        }
      }
    }
    changed |= preset.remove("left_ratio").is_some();
    changed |= preset.remove("right_ratio").is_some();
  }
  Ok(changed)
}

fn ratio(value: Option<&Value>, default: u16) -> Result<u16> {
  match value {
    Some(value) => Ok(
      u16::try_from(
        value
          .as_integer()
          .context("panel ratio must be an integer")?,
      )
      .context("panel ratio must fit in u16")?
      .max(1),
    ),
    None => Ok(default),
  }
}

/// Resolve old preset references once, then persist independent child panels.
fn inline_panels(value: &mut Value) -> Result<bool> {
  let Some(layout) = value.get_mut("layout").and_then(Value::as_table_mut) else {
    return Ok(false);
  };
  let active = layout
    .get("active")
    .and_then(Value::as_str)
    .unwrap_or("")
    .to_owned();
  let Some(presets) = layout.get_mut("presets").and_then(Value::as_table_mut) else {
    return Ok(false);
  };
  let defaults = Value::try_from(super::layout::LayoutConfig::default())?;
  let mut available = defaults["presets"]
    .as_table()
    .context("missing default presets")?
    .clone();
  for (name, parent, side) in LEGACY_PANELS {
    available.insert(
      (*name).into(),
      defaults["presets"][*parent]["composition"][*side].clone(),
    );
  }
  for (name, preset) in presets.iter() {
    let mut merged = available
      .get(name)
      .and_then(Value::as_table)
      .cloned()
      .unwrap_or_default();
    merged.extend(preset.as_table().context("preset must be a table")?.clone());
    available.insert(name.clone(), Value::Table(merged));
  }
  let mut changed = false;
  for (_, preset) in presets.iter_mut() {
    let Some(split) = preset.get_mut("composition").and_then(Value::as_table_mut) else {
      continue;
    };
    for side in ["first", "second"] {
      let Some(panel) = split.get_mut(side) else {
        continue;
      };
      let Some(name) = panel.get("layout").and_then(Value::as_str) else {
        continue;
      };
      let mut child = available
        .get(name)
        .with_context(|| format!("unknown panel preset: {name}"))?
        .as_table()
        .context("panel preset must be a table")?
        .clone();
      anyhow::ensure!(
        !child.contains_key("composition"),
        "nested compositions are not supported"
      );
      let args: Vec<String> = panel
        .get("args")
        .cloned()
        .unwrap_or(Value::Array(Vec::new()))
        .try_into()?;
      let params: Vec<String> = child
        .get("params")
        .cloned()
        .unwrap_or(Value::Array(Vec::new()))
        .try_into()?;
      anyhow::ensure!(
        args.len() <= params.len(),
        "too many panel arguments for {name}"
      );
      for (param, arg) in params.iter().zip(&args) {
        let key = panel_parameter(param);
        let v = match key.as_str() {
          "highlight" | "show_page_numbers" | "show_border" | "show_filename" => {
            Value::Boolean(match arg.trim().to_ascii_lowercase().as_str() {
              "true" | "yes" | "on" | "1" => true,
              "false" | "no" | "off" | "0" => false,
              _ => anyhow::bail!("invalid boolean panel argument: {arg}"),
            })
          }
          "image_ratio" => Value::Float(arg.parse()?),
          "card_style" | "filename_position" | "image_alignment" => Value::String(arg.clone()),
          _ => Value::Integer(
            arg
              .parse()
              .with_context(|| format!("invalid panel argument {param}: {arg}"))?,
          ),
        };
        child.insert(key, v);
      }
      child.remove("params");
      *panel = Value::Table(child);
      changed = true;
    }
  }
  // A helper explicitly selected as a standalone layout is a user choice;
  // keep it until it is no longer the startup layout rather than losing state.
  for (name, _, _) in LEGACY_PANELS {
    if active != *name {
      changed |= presets.remove(*name).is_some();
    }
  }
  Ok(changed)
}

fn panel_parameter(param: &str) -> String {
  let key = param.trim().to_ascii_lowercase().replace('-', "_");
  match key.as_str() {
    "column" | "cols" => "columns",
    "row" => "rows",
    "item" | "page_size" | "per_page" => "items",
    "width" | "w" => "card_width",
    "height" | "h" => "card_height",
    "divisor" | "step" | "chunk" => "scroll_divisor",
    "style" => "card_style",
    "name_position" => "filename_position",
    "image_align" | "align" => "image_alignment",
    "image_size" | "ratio" => "image_ratio",
    "name_lines" => "label_lines",
    "border" | "borders" => "show_border",
    "pad" => "padding",
    "name" => "show_filename",
    _ => return key,
  }
  .into()
}

const LEGACY_PANELS: &[(&str, &str, &str)] = &[
  ("slides-sidebar", "slides", "first"),
  ("slides-main", "slides", "second"),
];
