use std::{
  fs::File,
  path::{Path, PathBuf},
  time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use tracing::Level;
use tracing_subscriber::fmt;

pub fn init(cache_dir: &Path) -> Result<PathBuf> {
  let log_dir = cache_dir.join("logs");
  std::fs::create_dir_all(&log_dir)
    .with_context(|| format!("failed to create {}", log_dir.display()))?;
  let started = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs();
  let log_path = log_dir.join(format!("{started}.log"));
  let file =
    File::create(&log_path).with_context(|| format!("failed to create {}", log_path.display()))?;

  fmt()
    .with_writer(file)
    .with_ansi(false)
    .with_target(true)
    .with_thread_ids(true)
    .with_level(true)
    .with_max_level(Level::DEBUG)
    .init();

  Ok(log_path)
}
