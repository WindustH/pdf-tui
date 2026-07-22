mod behavior;
mod keymap;
mod layout;
mod render;
mod theme;

use std::{
  collections::BTreeSet,
  env,
  fmt::Write as FmtWrite,
  path::{Path, PathBuf},
  time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::fs;

pub use behavior::BehaviorConfig;
pub use keymap::KeymapConfig;
use keymap::format_keymap_toml;
pub use layout::{EffectiveLayoutConfig, LayoutConfig};
pub use render::{PdfRasterBackend, RenderConfig};
pub use theme::ThemeConfig;

#[derive(Debug, Clone)]
pub struct Settings {
  pub config: AppConfig,
  pub keymap: KeymapConfig,
  pub theme: ThemeConfig,
  pub config_path: PathBuf,
  pub cache_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AppConfig {
  pub layout: LayoutConfig,
  pub render: RenderConfig,
  pub behavior: BehaviorConfig,
}

impl AppConfig {
  fn normalize_defaults(&mut self) {
    self.layout.normalize_defaults();
    self.render.normalize_defaults();
    self.behavior.normalize_defaults();
  }
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
  let body = toml::to_string_pretty(config)?;
  Ok(add_app_config_comments(
    &body,
    &[
      "pdf-tui main configuration.",
      "Missing fields are rewritten with defaults when the app loads this file.",
    ],
    pdf_config_comment,
  ))
}

fn add_app_config_comments(
  body: &str,
  header: &[&str],
  comment_for: fn(&str) -> Option<&'static str>,
) -> String {
  let mut out = String::new();
  let mut seen_comments = BTreeSet::new();
  for line in header {
    push_toml_comment(&mut out, line);
  }
  out.push('\n');

  let mut table = String::new();
  for line in body.lines() {
    let trimmed = line.trim();
    if let Some(header) = toml_table_header(trimmed) {
      table = header.to_string();
      let comment_key = comment_table_key(&table);
      if seen_comments.insert(comment_key.clone())
        && let Some(comment) = comment_for(&comment_key)
      {
        push_toml_comment(&mut out, comment);
      }
    } else if let Some(key) = toml_field_key(trimmed) {
      let comment_key = comment_field_key(&table, key);
      if seen_comments.insert(comment_key.clone())
        && let Some(comment) = comment_for(&comment_key)
      {
        push_toml_comment(&mut out, comment);
      }
    }
    out.push_str(line);
    out.push('\n');
  }
  out
}

fn push_toml_comment(out: &mut String, comment: &str) {
  for line in comment.lines() {
    let _ = writeln!(out, "# {line}");
  }
}

fn toml_table_header(line: &str) -> Option<&str> {
  if line.starts_with("[[") {
    return None;
  }
  line.strip_prefix('[')?.strip_suffix(']')
}

fn toml_field_key(line: &str) -> Option<&str> {
  if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
    return None;
  }
  let (key, _) = line.split_once('=')?;
  let key = key.trim();
  (!key.is_empty()).then_some(key)
}

fn comment_table_key(table: &str) -> String {
  if table.starts_with("layout.presets.") {
    "layout.presets.*".to_string()
  } else {
    table.to_string()
  }
}

fn comment_field_key(table: &str, key: &str) -> String {
  if table.is_empty() {
    key.to_string()
  } else if table.starts_with("layout.presets.") {
    format!("layout.presets.*.{key}")
  } else {
    format!("{table}.{key}")
  }
}

fn pdf_config_comment(key: &str) -> Option<&'static str> {
  match key {
    "layout" => Some("Layout defaults and the startup preset selection."),
    "layout.active" => Some("Layout preset selected on startup."),
    "layout.active_args" => Some(
      "Arguments passed to the active layout preset, in the order declared by that preset's params.",
    ),
    "layout.gap_x" => Some("Default horizontal gap between rendered pages."),
    "layout.gap_y" => Some("Default vertical gap between rendered pages."),
    "layout.show_border" => Some("Draw borders around rendered pages by default."),
    "layout.padding" => Some("Default inner padding around rendered pages."),
    "layout.presets.*" => Some("Named layout preset used by the :layout command."),
    "layout.presets.*.strategy" => Some("Layout algorithm used by this preset: scroll or grid."),
    "layout.presets.*.params" => Some("Runtime arguments accepted by :layout for this preset."),
    "layout.presets.*.columns" => Some("Default number of page columns."),
    "layout.presets.*.rows" => Some("Default number of page rows for grid layouts."),
    "layout.presets.*.scroll_divisor" => {
      Some("Scroll step divisor for continuous scrolling layouts.")
    }
    "layout.presets.*.gap_x" => Some("Override the global horizontal page gap for this preset."),
    "layout.presets.*.gap_y" => Some("Override the global vertical page gap for this preset."),
    "layout.presets.*.show_border" => Some("Override whether this preset draws page borders."),
    "layout.presets.*.padding" => Some("Override the page padding for this preset."),
    "render" => Some("PDF conversion, terminal rendering, preloading, and cache settings."),
    "render.pdf_raster_backend" => Some("PDF page raster backend: poppler, mutool, or pdfium."),
    "render.pdf_raster_batch_pages" => Some("Number of PDF pages requested per raster batch."),
    "render.pdfinfo_bin" => Some("Command used to read PDF metadata."),
    "render.pdftoppm_bin" => Some("Command used for the poppler raster backend."),
    "render.mutool_bin" => Some("Command used for the mutool raster backend."),
    "render.mutool_band_height" => {
      Some("Band height passed to mutool draw -B for threaded rendering.")
    }
    "render.mutool_threads" => Some("Rendering threads passed to mutool draw -T."),
    "render.mutool_parallel" => Some("Enable mutool draw -P parallel interpretation/rendering."),
    "render.pdfium_library_path" => Some("Optional path to libpdfium or its containing directory."),
    "render.pdftk_bin" => Some("Command used for PDF toolkit operations when available."),
    "render.pdftotext_bin" => Some("Command used to extract searchable text from PDFs."),
    "render.page_dpi" => Some("DPI used when rasterizing PDF pages before terminal rendering."),
    "render.chafa_bin" => Some("Command used to render rasterized pages in the terminal."),
    "render.auto_detect" => {
      Some("Detect terminal graphics capability and adjust Chafa arguments automatically.")
    }
    "render.chafa_args" => {
      Some("Extra arguments passed to Chafa after terminal auto-detection is applied.")
    }
    "render.cache_max_bytes" => Some("Maximum disk space used for the render cache."),
    "render.cache_compression_level" => Some("Compression level used for cached render data."),
    "render.cache_compression_threads" => {
      Some("Worker threads used when compressing cache entries.")
    }
    "render.memory_compression" => Some("Compress rendered page data kept in memory."),
    "render.raw_memory_cache_max_bytes" => {
      Some("Maximum RAM used for uncompressed rendered page data.")
    }
    "render.compressed_memory_cache_max_bytes" => {
      Some("Maximum RAM used for compressed rendered page data.")
    }
    "render.prepared_memory_cache_max_bytes" => {
      Some("Maximum RAM used for prepared page render data.")
    }
    "render.search_highlight_cache_max_bytes" => {
      Some("Maximum RAM used for rendered search highlights.")
    }
    "render.selection_cache_max_bytes" => {
      Some("Maximum disk space used for selection marker and crop PNGs.")
    }
    "render.selection_image_max_pixels" => {
      Some("Maximum pixel count for the copied selection image.")
    }
    "render.search_preload_idle_ms" => {
      Some("Delay after search text input before preloading search previews.")
    }
    "render.max_concurrent" => Some("Maximum number of page render jobs running concurrently."),
    "render.chafa_threads" => Some("Threads requested per Chafa render job."),
    "render.preload_ahead" => Some("Number of pages ahead of the current page to preload."),
    "render.preload_behind" => Some("Number of pages behind the current page to keep preloaded."),
    "render.preload_slice_ahead" => Some("Number of page slices ahead to prepare."),
    "render.preload_slice_behind" => Some("Number of page slices behind to keep prepared."),
    "render.preload_terminal_ahead" => Some("Number of terminal-ready pages ahead to prepare."),
    "render.preload_terminal_behind" => {
      Some("Number of terminal-ready pages behind to keep prepared.")
    }
    "render.passthrough" => Some("Optional Chafa passthrough mode, such as tmux."),
    "render.zellij_sixel" => Some("Zellij SIXEL handling mode."),
    "behavior" => Some("Interactive behavior settings."),
    "behavior.scroll_lines" => Some("Rows moved by one wheel or scroll key step."),
    "behavior.frame_sync_navigation_viewer" => {
      Some("Keep the viewer frame synchronized while navigating.")
    }
    "behavior.frame_sync_navigation_bookmarks" => {
      Some("Keep the bookmarks frame synchronized while navigating.")
    }
    "behavior.frame_sync_navigation_search" => {
      Some("Keep the search frame synchronized while navigating.")
    }
    "behavior.auto_refresh" => Some("Automatically refresh the PDF when the file changes."),
    "behavior.auto_refresh_poll_ms" => Some("Polling interval for detecting PDF file changes."),
    "behavior.auto_refresh_min_interval_ms" => Some("Minimum delay between automatic refreshes."),
    "behavior.bookmarks_left_ratio" => Some("Left pane width ratio in the bookmarks view."),
    "behavior.bookmarks_right_ratio" => Some("Right pane width ratio in the bookmarks view."),
    "behavior.search_left_ratio" => Some("Left pane width ratio in the search view."),
    "behavior.search_right_ratio" => Some("Right pane width ratio in the search view."),
    _ => None,
  }
}

async fn read_or_write_keymap_default(path: &Path, default: KeymapConfig) -> Result<KeymapConfig> {
  if !path.exists() {
    return write_keymap_default(path, default).await;
  }
  let body = fs::read_to_string(path)
    .await
    .with_context(|| format!("failed to read {}", path.display()))?;
  let mut parsed: KeymapConfig = match toml::from_str(&body) {
    Ok(parsed) => parsed,
    Err(_) => return backup_and_write_keymap_default(path, default).await,
  };
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
    return write_default_config(path, default).await;
  }
  let body = fs::read_to_string(path)
    .await
    .with_context(|| format!("failed to read {}", path.display()))?;
  let mut parsed: T = match toml::from_str(&body) {
    Ok(parsed) => parsed,
    Err(_) => return backup_and_write_default_config(path, default).await,
  };
  parsed.normalize_defaults();
  if parsed.validate().is_err() {
    return backup_and_write_default_config(path, default).await;
  }
  let normalized = parsed.to_config_toml()?;
  write_back_if_toml_changed(path, &body, &normalized).await?;
  Ok(parsed)
}

trait NormalizeConfigDefaults {
  fn normalize_defaults(&mut self);

  fn validate(&self) -> Result<(), String> {
    Ok(())
  }

  fn to_config_toml(&self) -> Result<String>
  where
    Self: Serialize + Sized,
  {
    toml::to_string_pretty(self).map_err(Into::into)
  }
}

impl NormalizeConfigDefaults for AppConfig {
  fn normalize_defaults(&mut self) {
    AppConfig::normalize_defaults(self);
  }

  fn validate(&self) -> Result<(), String> {
    self.layout.validate()
  }

  fn to_config_toml(&self) -> Result<String> {
    app_config_toml(self)
  }
}

impl NormalizeConfigDefaults for ThemeConfig {
  fn normalize_defaults(&mut self) {
    ThemeConfig::normalize_defaults(self);
  }
}

async fn write_keymap_default(path: &Path, default: KeymapConfig) -> Result<KeymapConfig> {
  fs::write(path, format_keymap_toml(&default))
    .await
    .with_context(|| format!("failed to write {}", path.display()))?;
  Ok(default)
}

async fn backup_and_write_keymap_default(
  path: &Path,
  default: KeymapConfig,
) -> Result<KeymapConfig> {
  backup_config_file(path).await?;
  write_keymap_default(path, default).await
}

async fn write_default_config<T>(path: &Path, mut default: T) -> Result<T>
where
  T: Serialize + NormalizeConfigDefaults,
{
  default.normalize_defaults();
  let body = default.to_config_toml()?;
  fs::write(path, body)
    .await
    .with_context(|| format!("failed to write {}", path.display()))?;
  Ok(default)
}

async fn backup_and_write_default_config<T>(path: &Path, default: T) -> Result<T>
where
  T: Serialize + NormalizeConfigDefaults,
{
  backup_config_file(path).await?;
  write_default_config(path, default).await
}

async fn backup_config_file(path: &Path) -> Result<PathBuf> {
  let backup_path = next_backup_path(path);
  fs::rename(path, &backup_path).await.with_context(|| {
    format!(
      "failed to back up incompatible config {} to {}",
      path.display(),
      backup_path.display()
    )
  })?;
  Ok(backup_path)
}

fn next_backup_path(path: &Path) -> PathBuf {
  let file_name = path
    .file_name()
    .and_then(|name| name.to_str())
    .unwrap_or("config.toml");
  let stamp = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs();
  for index in 0..1000 {
    let suffix = if index == 0 {
      format!(".bak.{stamp}")
    } else {
      format!(".bak.{stamp}.{index}")
    };
    let candidate = path.with_file_name(format!("{file_name}{suffix}"));
    if !candidate.exists() {
      return candidate;
    }
  }
  path.with_file_name(format!("{file_name}.bak.{stamp}.overflow"))
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

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn app_config_toml_writes_parseable_commented_defaults() {
    let body = app_config_toml(&AppConfig::default()).expect("default config should serialize");
    toml::from_str::<AppConfig>(&body).expect("commented default config should parse");
    assert!(body.contains("# pdf-tui main configuration."));
    assert!(body.contains("# Layout preset selected on startup."));
  }

  #[test]
  fn app_config_toml_deduplicates_preset_field_comments() {
    let body = app_config_toml(&AppConfig::default()).expect("default config should serialize");
    assert_eq!(
      body
        .matches("# Layout algorithm used by this preset")
        .count(),
      1
    );
    assert_eq!(body.matches("# Default number of page columns.").count(), 1);
  }
}
