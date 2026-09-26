//! The on-disk cache shared by all pdf-tui processes: cross-process locks,
//! atomic writes with LRU markers, and size-limit cleanup.

mod cleanup;
mod files;
mod lock;

use std::time::{SystemTime, UNIX_EPOCH};

pub use cleanup::{
  CacheCleanupReport, clear_cache, enforce_cache_target_limit_sync, enforce_render_cache_limit,
  remove_legacy_crop_cache,
};
pub use files::{
  copy_file_atomic, touch_cache_entry, touch_cache_entry_sync, write_bytes_atomic,
  write_bytes_atomic_sync, write_file_atomic_sync,
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
