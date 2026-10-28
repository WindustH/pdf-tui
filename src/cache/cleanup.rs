use anyhow::{Context, Result};
pub use img_tui::cache::CacheStats as CacheCleanupReport;
use std::path::Path;

pub async fn enforce_render_cache_limit(
  cache_dir: &Path,
  max_bytes: u64,
) -> Result<CacheCleanupReport> {
  let session =
    img_tui::cache::paths::session(cache_dir).context("image cache is not initialized")?;
  tokio::task::spawn_blocking(move || session.database.trim(None, max_bytes)).await?
}
pub fn enforce_cache_target_limit_sync(
  cache_dir: &Path,
  target: &Path,
  max_bytes: u64,
) -> Result<CacheCleanupReport> {
  let session =
    img_tui::cache::paths::session(cache_dir).context("image cache is not initialized")?;
  session
    .database
    .trim(target.file_name().and_then(|v| v.to_str()), max_bytes)
}
pub async fn clear_cache(cache_dir: &Path) -> Result<CacheCleanupReport> {
  let session =
    img_tui::cache::paths::session(cache_dir).context("image cache is not initialized")?;
  tokio::task::spawn_blocking(move || session.clear()).await?
}
