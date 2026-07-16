use std::{
  fs::File,
  path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use tracing::Level;
use tracing_subscriber::fmt;

pub fn init(cache_dir: &Path) -> Result<PathBuf> {
  let log_dir = cache_dir.join("logs");
  std::fs::create_dir_all(&log_dir)
    .with_context(|| format!("failed to create {}", log_dir.display()))?;
  remove_old_logs(&log_dir)?;
  let log_path = log_dir.join("latest.log");
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

fn remove_old_logs(log_dir: &Path) -> Result<()> {
  for entry in
    std::fs::read_dir(log_dir).with_context(|| format!("failed to read {}", log_dir.display()))?
  {
    let path = entry
      .with_context(|| format!("failed to read entry in {}", log_dir.display()))?
      .path();
    if path.extension().is_some_and(|extension| extension == "log") {
      let _ = std::fs::remove_file(&path);
    }
  }
  Ok(())
}
