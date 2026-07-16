use std::{
  fs as std_fs,
  path::{Path, PathBuf},
  time::SystemTime,
};

use anyhow::{Context, Result};
use tokio::fs;

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

pub async fn enforce_render_cache_limit(
  cache_dir: &Path,
  max_bytes: u64,
) -> Result<CacheCleanupReport> {
  if max_bytes == 0 {
    return Ok(CacheCleanupReport::default());
  }

  let mut entries = collect_cache_entries(cache_dir).await?;
  let before_bytes = entries.iter().map(|entry| entry.size_bytes).sum::<u64>();
  if before_bytes <= max_bytes {
    return Ok(CacheCleanupReport {
      before_bytes,
      after_bytes: before_bytes,
      removed_files: 0,
      removed_bytes: 0,
    });
  }

  entries.sort_by(|left, right| {
    left
      .last_used
      .cmp(&right.last_used)
      .then_with(|| left.path.cmp(&right.path))
  });

  let mut after_bytes = before_bytes;
  let mut removed_files = 0;
  let mut removed_bytes = 0;
  for entry in entries {
    if after_bytes <= max_bytes {
      break;
    }
    if fs::remove_file(&entry.path).await.is_ok() {
      let _ = fs::remove_file(cache_used_path(&entry.path)).await;
      after_bytes = after_bytes.saturating_sub(entry.size_bytes);
      removed_files += 1;
      removed_bytes += entry.size_bytes;
    }
  }

  Ok(CacheCleanupReport {
    before_bytes,
    after_bytes,
    removed_files,
    removed_bytes,
  })
}

pub fn enforce_cache_target_limit_sync(
  cache_dir: &Path,
  target: &Path,
  max_bytes: u64,
) -> Result<CacheCleanupReport> {
  if max_bytes == 0 {
    return Ok(CacheCleanupReport::default());
  }
  let mut entries = collect_cache_entries_in_sync(cache_dir, target)?;
  let before_bytes = entries.iter().map(|entry| entry.size_bytes).sum::<u64>();
  if before_bytes <= max_bytes {
    return Ok(CacheCleanupReport {
      before_bytes,
      after_bytes: before_bytes,
      removed_files: 0,
      removed_bytes: 0,
    });
  }

  entries.sort_by(|left, right| {
    left
      .last_used
      .cmp(&right.last_used)
      .then_with(|| left.path.cmp(&right.path))
  });

  let mut after_bytes = before_bytes;
  let mut removed_files = 0;
  let mut removed_bytes = 0;
  for entry in entries {
    if after_bytes <= max_bytes {
      break;
    }
    if std_fs::remove_file(&entry.path).is_ok() {
      let _ = std_fs::remove_file(cache_used_path(&entry.path));
      after_bytes = after_bytes.saturating_sub(entry.size_bytes);
      removed_files += 1;
      removed_bytes += entry.size_bytes;
    }
  }

  Ok(CacheCleanupReport {
    before_bytes,
    after_bytes,
    removed_files,
    removed_bytes,
  })
}

pub async fn clear_cache(cache_dir: &Path) -> Result<CacheCleanupReport> {
  let targets = [
    cache_dir.join("pages"),
    cache_dir.join("render"),
    cache_dir.join("text"),
    cache_dir.join("search-highlight"),
  ];
  let mut before_bytes = 0;
  let mut before_files = 0;
  for target in &targets {
    let metrics = cache_tree_metrics(target).await?;
    before_bytes += metrics.bytes;
    before_files += metrics.files;
  }

  for target in &targets {
    remove_cache_target(target).await?;
    fs::create_dir_all(target)
      .await
      .with_context(|| format!("failed to create {}", target.display()))?;
  }

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

pub async fn remove_legacy_crop_cache(cache_dir: &Path) -> Result<()> {
  let crops = cache_dir.join("render").join("crops");
  match fs::remove_dir_all(&crops).await {
    Ok(()) => Ok(()),
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
    Err(error) => Err(error).with_context(|| format!("failed to remove {}", crops.display())),
  }
}

async fn collect_cache_entries(cache_dir: &Path) -> Result<Vec<CacheEntry>> {
  let mut entries = Vec::new();
  collect_cache_entries_in(cache_dir, cache_dir, &mut entries).await?;
  Ok(entries)
}

#[derive(Debug, Clone, Copy, Default)]
struct CacheTreeMetrics {
  files: usize,
  bytes: u64,
}

async fn cache_tree_metrics(path: &Path) -> Result<CacheTreeMetrics> {
  let metadata = match fs::metadata(path).await {
    Ok(metadata) => metadata,
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
      return Ok(CacheTreeMetrics::default());
    }
    Err(error) => {
      return Err(error).with_context(|| format!("failed to stat {}", path.display()));
    }
  };
  if metadata.is_file() {
    return Ok(CacheTreeMetrics {
      files: 1,
      bytes: metadata.len(),
    });
  }
  if !metadata.is_dir() {
    return Ok(CacheTreeMetrics::default());
  }

  let mut metrics = CacheTreeMetrics::default();
  let mut pending = vec![path.to_path_buf()];
  while let Some(dir) = pending.pop() {
    let mut reader = match fs::read_dir(&dir).await {
      Ok(reader) => reader,
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
      Err(error) => {
        return Err(error).with_context(|| format!("failed to read {}", dir.display()));
      }
    };
    while let Some(entry) = reader
      .next_entry()
      .await
      .with_context(|| format!("failed to scan {}", dir.display()))?
    {
      let path = entry.path();
      let metadata = match entry.metadata().await {
        Ok(metadata) => metadata,
        Err(_) => continue,
      };
      if metadata.is_dir() {
        pending.push(path);
      } else if metadata.is_file() {
        metrics.files += 1;
        metrics.bytes += metadata.len();
      }
    }
  }
  Ok(metrics)
}

async fn remove_cache_target(path: &Path) -> Result<()> {
  match fs::remove_dir_all(path).await {
    Ok(()) => Ok(()),
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
    Err(error) => Err(error).with_context(|| format!("failed to remove {}", path.display())),
  }
}

async fn collect_cache_entries_in(
  cache_dir: &Path,
  dir: &Path,
  entries: &mut Vec<CacheEntry>,
) -> Result<()> {
  let mut pending = vec![dir.to_path_buf()];
  while let Some(dir) = pending.pop() {
    let mut reader = match fs::read_dir(&dir).await {
      Ok(reader) => reader,
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
      Err(error) => {
        return Err(error).with_context(|| format!("failed to read {}", dir.display()));
      }
    };

    while let Some(entry) = reader
      .next_entry()
      .await
      .with_context(|| format!("failed to scan {}", dir.display()))?
    {
      let path = entry.path();
      let metadata = match entry.metadata().await {
        Ok(metadata) => metadata,
        Err(_) => continue,
      };
      if metadata.is_dir() {
        pending.push(path);
        continue;
      }
      if !metadata.is_file() || !is_cache_payload(cache_dir, &path) {
        continue;
      }
      let last_used = cache_last_used(&path, &metadata).await;
      entries.push(CacheEntry {
        path,
        size_bytes: metadata.len(),
        last_used,
      });
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
    }
    Some("zst") => path.starts_with(cache_dir.join("text")),
    _ => false,
  }
}

pub async fn touch_cache_entry(cache_path: &Path) {
  let _ = fs::write(cache_used_path(cache_path), []).await;
}

pub fn touch_cache_entry_sync(cache_path: &Path) {
  let _ = std_fs::write(cache_used_path(cache_path), []);
}

fn cache_used_path(cache_path: &Path) -> PathBuf {
  let mut path = cache_path.to_path_buf();
  let extension = cache_path
    .extension()
    .and_then(|value| value.to_str())
    .map(|extension| format!("{extension}.used"))
    .unwrap_or_else(|| "used".to_string());
  path.set_extension(extension);
  path
}

async fn cache_last_used(cache_path: &Path, metadata: &std::fs::Metadata) -> SystemTime {
  if let Ok(used_metadata) = fs::metadata(cache_used_path(cache_path)).await
    && let Ok(modified) = used_metadata.modified()
  {
    return modified;
  }
  metadata
    .accessed()
    .or_else(|_| metadata.modified())
    .unwrap_or(SystemTime::UNIX_EPOCH)
}

fn collect_cache_entries_in_sync(cache_dir: &Path, dir: &Path) -> Result<Vec<CacheEntry>> {
  let mut entries = Vec::new();
  let mut pending = vec![dir.to_path_buf()];
  while let Some(dir) = pending.pop() {
    let reader = match std_fs::read_dir(&dir) {
      Ok(reader) => reader,
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
      Err(error) => {
        return Err(error).with_context(|| format!("failed to read {}", dir.display()));
      }
    };

    for entry in reader {
      let path = entry
        .with_context(|| format!("failed to scan {}", dir.display()))?
        .path();
      let metadata = match std_fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(_) => continue,
      };
      if metadata.is_dir() {
        pending.push(path);
        continue;
      }
      if !metadata.is_file() || !is_cache_payload(cache_dir, &path) {
        continue;
      }
      let last_used = cache_last_used_sync(&path, &metadata);
      entries.push(CacheEntry {
        path,
        size_bytes: metadata.len(),
        last_used,
      });
    }
  }
  Ok(entries)
}

fn cache_last_used_sync(cache_path: &Path, metadata: &std::fs::Metadata) -> SystemTime {
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
