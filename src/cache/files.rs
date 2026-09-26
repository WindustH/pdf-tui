//! Atomic cache writes and the `.used` marker files that record when an
//! entry was last used, for LRU trimming.

use std::{
  collections::HashMap,
  fs as std_fs,
  path::{Path, PathBuf},
  sync::{Mutex, PoisonError},
  time::{Duration, Instant},
};

use anyhow::{Context, Result};
use tokio::fs;

use super::now_nanos;

/// How long a refreshed `.used` marker is considered current. Entries are
/// read many times per second while browsing; rewriting the marker each
/// time costs a create and a rename, while trimming only needs a rough
/// age.
const TOUCH_INTERVAL: Duration = Duration::from_secs(60);

static RECENT_TOUCHES: Mutex<Option<HashMap<PathBuf, Instant>>> = Mutex::new(None);

/// Records that `cache_path` was just used.
pub async fn touch_cache_entry(cache_path: &Path) {
  if claim_touch(cache_path) {
    let _ = write_bytes_atomic(&cache_used_path(cache_path), &[]).await;
  }
}

/// Blocking variant of [`touch_cache_entry`].
pub fn touch_cache_entry_sync(cache_path: &Path) {
  if claim_touch(cache_path) {
    let _ = write_bytes_atomic_sync(&cache_used_path(cache_path), &[]);
  }
}

/// Whether the marker for `cache_path` is due for a refresh; claims it.
fn claim_touch(cache_path: &Path) -> bool {
  let mut guard = RECENT_TOUCHES
    .lock()
    .unwrap_or_else(PoisonError::into_inner);
  let touches = guard.get_or_insert_with(HashMap::new);
  let now = Instant::now();
  if touches
    .get(cache_path)
    .is_some_and(|last| now.duration_since(*last) < TOUCH_INTERVAL)
  {
    return false;
  }
  touches.insert(cache_path.to_path_buf(), now);
  true
}

/// Forgets which markers were refreshed, e.g. after the cache was cleared.
pub(super) fn forget_touches() {
  let mut guard = RECENT_TOUCHES
    .lock()
    .unwrap_or_else(PoisonError::into_inner);
  *guard = None;
}

pub(super) fn cache_used_path(cache_path: &Path) -> PathBuf {
  let mut path = cache_path.to_path_buf();
  let extension = cache_path
    .extension()
    .and_then(|value| value.to_str())
    .map(|extension| format!("{extension}.used"))
    .unwrap_or_else(|| "used".to_string());
  path.set_extension(extension);
  path
}

pub async fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent)
      .await
      .with_context(|| format!("failed to create {}", parent.display()))?;
  }
  let temp_path = temp_sibling_path(path);
  let written = fs::write(&temp_path, bytes)
    .await
    .with_context(|| format!("failed to write {}", temp_path.display()));
  finish_temp_file(written, temp_path, path.to_path_buf()).await
}

pub fn write_bytes_atomic_sync(path: &Path, bytes: &[u8]) -> Result<()> {
  if let Some(parent) = path.parent() {
    std_fs::create_dir_all(parent)
      .with_context(|| format!("failed to create {}", parent.display()))?;
  }
  let temp_path = temp_sibling_path(path);
  std_fs::write(&temp_path, bytes)
    .with_context(|| format!("failed to write {}", temp_path.display()))
    .and_then(|()| persist_temp_file_sync(&temp_path, path))
    .inspect_err(|_| {
      let _ = std_fs::remove_file(&temp_path);
    })
}

/// Creates `path` through `write`, which fills a temporary sibling that is
/// then moved into place. The temporary file is removed on failure.
pub fn write_file_atomic_sync<E: std::fmt::Display>(
  path: &Path,
  write: impl FnOnce(&Path) -> Result<(), E>,
) -> Result<(), String> {
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
  replace_file(temp_path, output_path)
}

#[cfg(not(windows))]
fn replace_file(temp_path: &Path, output_path: &Path) -> Result<()> {
  std_fs::rename(temp_path, output_path).with_context(|| {
    format!(
      "failed to atomically replace {} with {}",
      output_path.display(),
      temp_path.display()
    )
  })
}

/// `std::fs::rename` cannot be relied on to overwrite an open destination
/// on Windows; `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` does.
#[cfg(windows)]
fn replace_file(temp_path: &Path, output_path: &Path) -> Result<()> {
  use std::os::windows::ffi::OsStrExt;
  use windows_sys::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
  };

  let wide = |path: &Path| {
    path
      .as_os_str()
      .encode_wide()
      .chain(std::iter::once(0))
      .collect::<Vec<_>>()
  };
  let source = wide(temp_path);
  let target = wide(output_path);
  let flags = MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH;
  let replaced = unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), flags) };
  if replaced != 0 {
    return Ok(());
  }
  Err(std::io::Error::last_os_error()).with_context(|| {
    format!(
      "failed to atomically replace {} with {}",
      output_path.display(),
      temp_path.display()
    )
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn used_markers_are_refreshed_at_most_once_per_interval() {
    let path = std::env::temp_dir().join(format!(
      "pdf-tui-touch-test-{}-{}.png",
      std::process::id(),
      now_nanos()
    ));
    assert!(claim_touch(&path));
    assert!(!claim_touch(&path));
    forget_touches();
    assert!(claim_touch(&path));
    assert_eq!(
      cache_used_path(&path).extension().unwrap(),
      "used",
      "marker keeps the payload extension before .used"
    );
  }
}
