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
  let multiple_instances = crate::cache::active_instance_count(cache_dir) > 1;
  if !multiple_instances {
    remove_old_logs(&log_dir)?;
  }
  let log_path = log_dir.join(format!("run-{}-{}.log", std::process::id(), now_nanos()));
  let file =
    File::create(&log_path).with_context(|| format!("failed to create {}", log_path.display()))?;
  update_latest_log_link(&log_dir, &log_path);

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
    if path.extension().is_some_and(|extension| extension == "log")
      || path.file_name().is_some_and(|name| name == "latest.log")
    {
      let _ = std::fs::remove_file(&path);
    }
  }
  Ok(())
}

fn update_latest_log_link(log_dir: &Path, log_path: &Path) {
  let latest = log_dir.join("latest.log");
  #[cfg(unix)]
  {
    use std::os::unix::fs::symlink;
    let target = log_path
      .file_name()
      .map(PathBuf::from)
      .unwrap_or_else(|| log_path.to_path_buf());
    let temp = crate::cache::temp_sibling_path(&latest);
    let _ = std::fs::remove_file(&temp);
    if symlink(target, &temp).is_ok() {
      let _ = std::fs::rename(&temp, &latest);
      let _ = std::fs::remove_file(&temp);
    }
  }
  #[cfg(not(unix))]
  {
    let _ = crate::cache::write_bytes_atomic_sync(&latest, log_path.to_string_lossy().as_bytes());
  }
}

fn now_nanos() -> u128 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_nanos()
}
