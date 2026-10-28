//! App-specific adapters for img-tui SQLite caches, render job locks, and
//! atomic writes of non-cache configuration/state files.

mod cleanup;
mod files;
mod lock;

use std::time::{SystemTime, UNIX_EPOCH};

pub use cleanup::{
  CacheCleanupReport, clear_cache, enforce_cache_target_limit_sync, enforce_render_cache_limit,
};
pub use files::{
  copy_file_atomic, write_bytes_atomic, write_bytes_atomic_sync, write_file_atomic_sync,
};
pub use lock::{
  CacheFileLock, acquire_cache_file_lock, acquire_cache_file_lock_sync, active_instance_count,
  register_instance,
};

fn now_nanos() -> u128 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_nanos()
}

pub use lock::acquire_document_lock;

/// A cached image is optional: unreadable materializations are evicted so the
/// caller can regenerate them under its normal cache lock.
pub fn valid_cached_image(path: &std::path::Path) -> bool {
  if !img_tui::cache::paths::contains(path) {
    return false;
  }
  let materialized = img_tui::cache::paths::materialize(path);
  if let Ok(file) = &materialized {
    if image::image_dimensions(file).is_ok() {
      return true;
    }
    let _ = std::fs::remove_file(file);
  }
  let _ = img_tui::cache::paths::remove(path);
  false
}
