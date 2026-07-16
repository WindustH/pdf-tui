mod behavior;
mod keymap;
mod layout;
mod render;
mod theme;

use std::{
  env,
  path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::fs;

pub use behavior::BehaviorConfig;
pub use keymap::KeymapConfig;
use keymap::format_keymap_toml;
pub use layout::{EffectiveLayoutConfig, LayoutConfig};
pub use render::RenderConfig;
pub use theme::ThemeConfig;

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
