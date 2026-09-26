//! Size limits and clearing. Cache payloads are trimmed least recently used
//! first, by the time of their `.used` marker.

use std::{
  collections::HashSet,
  fs::{self as std_fs, Metadata},
  io::ErrorKind,
  path::{Path, PathBuf},
  time::SystemTime,
};

use anyhow::{Context, Result, anyhow};
use tokio::fs;

use super::{
  files::{cache_used_path, forget_touches},
  lock::{acquire_cache_file_lock, active_instance_count},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheCleanupReport {
  pub before_bytes: u64,
  pub after_bytes: u64,
  pub removed_files: usize,
  pub removed_bytes: u64,
}

#[derive(Debug)]
struct CacheEntry {
  path: PathBuf,
  size_bytes: u64,
  last_used: SystemTime,
}

/// Directories holding regenerable cache data, relative to the cache root.
const CACHE_TARGETS: [&str; 5] = ["pages", "render", "text", "search-highlight", "selection"];

/// Trims the whole cache below `max_bytes` (0 disables the limit) and
/// removes bookkeeping files whose payload is gone. Skipped while another
/// instance runs, since it may still display the files.
pub async fn enforce_render_cache_limit(
  cache_dir: &Path,
  max_bytes: u64,
) -> Result<CacheCleanupReport> {
  if max_bytes == 0 || active_instance_count(cache_dir) > 1 {
    return Ok(CacheCleanupReport::default());
  }
  let cache_dir = cache_dir.to_path_buf();
  tokio::task::spawn_blocking(move || {
    let scan = scan_cache_tree(&cache_dir, &cache_dir)?;
    remove_orphans(&scan);
    Ok(evict_until_within(scan.entries, max_bytes))
  })
  .await
  .context("cache cleanup task failed")?
}

/// Trims one cache subdirectory below `max_bytes` after new entries were
/// written to it.
pub fn enforce_cache_target_limit_sync(
  cache_dir: &Path,
  target: &Path,
  max_bytes: u64,
) -> Result<CacheCleanupReport> {
  if max_bytes == 0 || active_instance_count(cache_dir) > 1 {
    return Ok(CacheCleanupReport::default());
  }
  Ok(evict_until_within(
    scan_cache_tree(cache_dir, target)?.entries,
    max_bytes,
  ))
}

fn evict_until_within(mut entries: Vec<CacheEntry>, max_bytes: u64) -> CacheCleanupReport {
  let before_bytes = entries.iter().map(|entry| entry.size_bytes).sum::<u64>();
  let mut report = CacheCleanupReport {
    before_bytes,
    after_bytes: before_bytes,
    ..CacheCleanupReport::default()
  };
  if before_bytes <= max_bytes {
    return report;
  }
  entries.sort_by(|left, right| {
    left
      .last_used
      .cmp(&right.last_used)
      .then_with(|| left.path.cmp(&right.path))
  });
  for entry in entries {
    if report.after_bytes <= max_bytes {
      break;
    }
    if std_fs::remove_file(&entry.path).is_ok() {
      remove_companions(&entry.path);
      report.after_bytes = report.after_bytes.saturating_sub(entry.size_bytes);
      report.removed_files += 1;
      report.removed_bytes += entry.size_bytes;
    }
  }
  report
}

/// Removes the files that only describe a payload: its `.used` marker and,
/// for page slices, the `.toml` slice metadata with its own marker.
fn remove_companions(payload: &Path) {
  let _ = std_fs::remove_file(cache_used_path(payload));
  if payload
    .extension()
    .is_some_and(|extension| extension == "png")
  {
    let metadata = payload.with_extension("toml");
    if std_fs::remove_file(&metadata).is_ok() {
      let _ = std_fs::remove_file(cache_used_path(&metadata));
    }
  }
}

pub async fn clear_cache(cache_dir: &Path) -> Result<CacheCleanupReport> {
  if active_instance_count(cache_dir) > 1 {
    return Err(anyhow!(
      "refusing to clear cache while another pdf-tui instance is running"
    ));
  }
  let _lock = acquire_cache_file_lock(&cache_dir.join("clear-cache")).await?;
  let targets = CACHE_TARGETS.map(|target| cache_dir.join(target));
  let mut before_bytes = 0;
  let mut before_files = 0;
  for target in &targets {
    let metrics = cache_tree_metrics(target).await?;
    before_bytes += metrics.bytes;
    before_files += metrics.files;
  }

  for target in &targets {
    match fs::remove_dir_all(target).await {
      Ok(()) => {}
      Err(error) if error.kind() == ErrorKind::NotFound => {}
      Err(error) => {
        return Err(error).with_context(|| format!("failed to remove {}", target.display()));
      }
    }
    fs::create_dir_all(target)
      .await
      .with_context(|| format!("failed to create {}", target.display()))?;
  }
  forget_touches();

  let mut after_bytes = 0;
  for target in &targets {
    after_bytes += cache_tree_metrics(target).await?.bytes;
  }

  Ok(CacheCleanupReport {
    before_bytes,
    after_bytes,
    removed_files: before_files,
    removed_bytes: before_bytes.saturating_sub(after_bytes),
  })
}

/// Deletes the crop cache used by releases before selection crops moved to
/// `selection/`.
pub async fn remove_legacy_crop_cache(cache_dir: &Path) -> Result<()> {
  if active_instance_count(cache_dir) > 1 {
    return Ok(());
  }
  let crops = cache_dir.join("render").join("crops");
  match fs::remove_dir_all(&crops).await {
    Ok(()) => Ok(()),
    Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
    Err(error) => Err(error).with_context(|| format!("failed to remove {}", crops.display())),
  }
}

#[derive(Debug, Clone, Copy, Default)]
struct CacheTreeMetrics {
  files: usize,
  bytes: u64,
}

async fn cache_tree_metrics(path: &Path) -> Result<CacheTreeMetrics> {
  let path = path.to_path_buf();
  tokio::task::spawn_blocking(move || {
    let mut metrics = CacheTreeMetrics::default();
    walk_files(&path, &mut |_, metadata| {
      metrics.files += 1;
      metrics.bytes += metadata.len();
    })?;
    Ok(metrics)
  })
  .await
  .context("cache scan task failed")?
}

/// Cache payloads under `dir`, plus the bookkeeping files found beside
/// them.
struct CacheScan {
  entries: Vec<CacheEntry>,
  bookkeeping: Vec<PathBuf>,
}

fn scan_cache_tree(cache_dir: &Path, dir: &Path) -> Result<CacheScan> {
  let mut scan = CacheScan {
    entries: Vec::new(),
    bookkeeping: Vec::new(),
  };
  walk_files(dir, &mut |path, metadata| {
    if is_cache_payload(cache_dir, path) {
      scan.entries.push(CacheEntry {
        path: path.to_path_buf(),
        size_bytes: metadata.len(),
        last_used: cache_last_used(path, metadata),
      });
    } else if is_bookkeeping(cache_dir, path) {
      scan.bookkeeping.push(path.to_path_buf());
    }
  })?;
  Ok(scan)
}

/// Removes `.used` markers and slice metadata left behind by payloads that
/// were deleted without their companions.
fn remove_orphans(scan: &CacheScan) {
  let payloads = scan
    .entries
    .iter()
    .map(|entry| entry.path.as_path())
    .collect::<HashSet<_>>();
  let owner_of = |path: &Path| {
    if path
      .extension()
      .is_some_and(|extension| extension == "used")
    {
      path.with_extension("")
    } else {
      path.with_extension("png")
    }
  };
  // Slice metadata whose PNG still exists; its marker stays too.
  let live_metadata = scan
    .bookkeeping
    .iter()
    .filter(|path| {
      path
        .extension()
        .is_some_and(|extension| extension == "toml")
    })
    .filter(|path| payloads.contains(owner_of(path).as_path()))
    .map(PathBuf::as_path)
    .collect::<HashSet<_>>();
  for path in &scan.bookkeeping {
    if is_temp_file(path) {
      if is_stale(path) {
        let _ = std_fs::remove_file(path);
      }
      continue;
    }
    let owner = owner_of(path);
    if !payloads.contains(owner.as_path()) && !live_metadata.contains(owner.as_path()) {
      let _ = std_fs::remove_file(path);
    }
  }
}

/// Temporary siblings (`name.tmp-<pid>-<nanos>`) survive only when a
/// writer died mid-write, so old ones are garbage; young ones may belong
/// to a write in progress.
fn is_stale(path: &Path) -> bool {
  const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(600);
  std_fs::metadata(path)
    .and_then(|metadata| metadata.modified())
    .ok()
    .and_then(|modified| modified.elapsed().ok())
    .is_some_and(|age| age > STALE_AFTER)
}

fn is_temp_file(path: &Path) -> bool {
  path
    .file_name()
    .and_then(|name| name.to_str())
    .is_some_and(|name| name.contains(".tmp-"))
}

fn walk_files(root: &Path, visit: &mut dyn FnMut(&Path, &Metadata)) -> Result<()> {
  if let Ok(metadata) = std_fs::metadata(root)
    && metadata.is_file()
  {
    visit(root, &metadata);
    return Ok(());
  }
  let mut pending = vec![root.to_path_buf()];
  while let Some(dir) = pending.pop() {
    let reader = match std_fs::read_dir(&dir) {
      Ok(reader) => reader,
      Err(error) if error.kind() == ErrorKind::NotFound => continue,
      Err(error) => {
        return Err(error).with_context(|| format!("failed to read {}", dir.display()));
      }
    };
    for entry in reader {
      let path = entry
        .with_context(|| format!("failed to scan {}", dir.display()))?
        .path();
      let Ok(metadata) = std_fs::metadata(&path) else {
        continue;
      };
      if metadata.is_dir() {
        pending.push(path);
      } else if metadata.is_file() {
        visit(&path, &metadata);
      }
    }
  }
  Ok(())
}

fn is_cache_payload(cache_dir: &Path, path: &Path) -> bool {
  match path.extension().and_then(|value| value.to_str()) {
    Some("ansi") => path.starts_with(cache_dir.join("render")),
    Some("png") => {
      path.starts_with(cache_dir.join("pages"))
        || path.starts_with(cache_dir.join("search-highlight"))
        || path.starts_with(cache_dir.join("selection"))
    }
    Some("zst") => path.starts_with(cache_dir.join("text")),
    _ => false,
  }
}

/// `.used` markers and temporary siblings anywhere in the cache targets,
/// and slice metadata in `pages/`.
fn is_bookkeeping(cache_dir: &Path, path: &Path) -> bool {
  let in_targets = CACHE_TARGETS
    .iter()
    .any(|target| path.starts_with(cache_dir.join(target)));
  if !in_targets {
    return false;
  }
  match path.extension().and_then(|value| value.to_str()) {
    Some("used") => true,
    Some("toml") => path.starts_with(cache_dir.join("pages")),
    _ => is_temp_file(path),
  }
}

fn cache_last_used(cache_path: &Path, metadata: &Metadata) -> SystemTime {
  if let Ok(used_metadata) = std_fs::metadata(cache_used_path(cache_path))
    && let Ok(modified) = used_metadata.modified()
  {
    return modified;
  }
  metadata
    .accessed()
    .or_else(|_| metadata.modified())
    .unwrap_or(SystemTime::UNIX_EPOCH)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn eviction_and_orphan_sweep_remove_slice_companions() {
    let root = std::env::temp_dir().join(format!("pdf-tui-cleanup-test-{}", std::process::id()));
    let _ = std_fs::remove_dir_all(&root);
    let pages = root.join("pages");
    std_fs::create_dir_all(&pages).unwrap();
    let write =
      |name: &str, bytes: usize| std_fs::write(pages.join(name), vec![0_u8; bytes]).unwrap();
    // An evictable slice with metadata, a kept page, and orphans of a slice
    // that disappeared earlier.
    write("old-slice.png", 100);
    write("old-slice.png.used", 0);
    write("old-slice.toml", 10);
    write("old-slice.toml.used", 0);
    write("keep.png", 100);
    write("keep.png.used", 0);
    write("gone-slice.toml", 10);
    write("gone-slice.png.used", 0);
    write("fresh.png.tmp-1-2", 5);
    std::thread::sleep(std::time::Duration::from_millis(20));
    std_fs::write(pages.join("keep.png.used"), []).unwrap();

    let scan = scan_cache_tree(&root, &root).unwrap();
    remove_orphans(&scan);
    assert!(!pages.join("gone-slice.toml").exists());
    assert!(!pages.join("gone-slice.png.used").exists());
    assert!(pages.join("old-slice.toml").exists());
    assert!(pages.join("fresh.png.tmp-1-2").exists());
    std_fs::remove_file(pages.join("fresh.png.tmp-1-2")).unwrap();

    let report = evict_until_within(scan.entries, 150);
    assert_eq!(report.removed_files, 1);
    let mut left = std_fs::read_dir(&pages)
      .unwrap()
      .map(|entry| entry.unwrap().file_name().into_string().unwrap())
      .collect::<Vec<_>>();
    left.sort();
    assert_eq!(left, vec!["keep.png", "keep.png.used"]);
    let _ = std_fs::remove_dir_all(&root);
  }
}
