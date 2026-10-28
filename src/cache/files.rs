//! SQLite writes for cache payloads; atomic file writes for configuration and state.

use std::{
  fs as std_fs,
  path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use tokio::fs;

use super::now_nanos;

pub async fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
  if img_tui::cache::paths::is_managed(path) {
    return img_tui::cache::paths::write(path, bytes).await;
  }
  let path = path.to_owned();
  let bytes = bytes.to_vec();
  tokio::task::spawn_blocking(move || framework_tui::fs::atomic_write_bytes(&path, &bytes))
    .await?
    .map_err(Into::into)
}

pub fn write_bytes_atomic_sync(path: &Path, bytes: &[u8]) -> Result<()> {
  if img_tui::cache::paths::is_managed(path) {
    return img_tui::cache::paths::write_sync(path, bytes);
  }
  framework_tui::fs::atomic_write_bytes(path, bytes).map_err(Into::into)
}

/// Creates `path` through `write`, which fills a temporary sibling that is
/// then moved into place. The temporary file is removed on failure.
pub fn write_file_atomic_sync<E: std::fmt::Display>(
  path: &Path,
  write: impl FnOnce(&Path) -> Result<(), E>,
) -> Result<(), String> {
  if img_tui::cache::paths::is_managed(path) {
    return img_tui::cache::paths::write_file_sync(path, write).map_err(|e| e.to_string());
  }
  let temp_path = temp_sibling_path(path);
  let result = write(&temp_path)
    .map_err(|error| format!("failed to write {}: {error}", temp_path.display()))
    .and_then(|()| {
      persist_temp_file_sync(&temp_path, path)
        .map_err(|error| format!("failed to move {}: {error}", path.display()))
    });
  if result.is_err() {
    let _ = std_fs::remove_file(&temp_path);
  }
  result
}

/// Copies `source` to `path` so that readers only ever see the complete
/// file.
pub async fn copy_file_atomic(source: &Path, path: &Path) -> Result<()> {
  if img_tui::cache::paths::is_managed(path) {
    return img_tui::cache::paths::write(path, &fs::read(source).await?).await;
  }
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent)
      .await
      .with_context(|| format!("failed to create {}", parent.display()))?;
  }
  let temp_path = temp_sibling_path(path);
  let copied = fs::copy(source, &temp_path)
    .await
    .map(|_| ())
    .with_context(|| {
      format!(
        "failed to copy {} to {}",
        source.display(),
        temp_path.display()
      )
    });
  finish_temp_file(copied, temp_path, path.to_path_buf()).await
}

/// Moves a fully written temporary sibling into place, or removes it when
/// writing failed.
async fn finish_temp_file(written: Result<()>, temp_path: PathBuf, path: PathBuf) -> Result<()> {
  tokio::task::spawn_blocking(move || {
    written
      .and_then(|()| persist_temp_file_sync(&temp_path, &path))
      .inspect_err(|_| {
        let _ = std_fs::remove_file(&temp_path);
      })
  })
  .await
  .context("cache file replace task failed")?
}

/// A unique temporary name next to `path`, on the same filesystem so the
/// final rename is atomic.
pub fn temp_sibling_path(path: &Path) -> PathBuf {
  let mut name = path
    .file_name()
    .map(|name| name.to_os_string())
    .unwrap_or_else(|| "cache".into());
  name.push(format!(".tmp-{}-{}", std::process::id(), now_nanos()));
  path.with_file_name(name)
}

/// Atomically replaces `output_path` with `temp_path`.
pub fn persist_temp_file_sync(temp_path: &Path, output_path: &Path) -> Result<()> {
  framework_tui::fs::replace_path(temp_path, output_path).map_err(Into::into)
}
