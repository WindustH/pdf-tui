use std::{
  fs::{self as std_fs, File, OpenOptions},
  io::{ErrorKind, Write},
  path::{Path, PathBuf},
  sync::OnceLock,
  time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
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

pub struct CacheInstanceGuard {
  path: PathBuf,
  _file: File,
}

impl Drop for CacheInstanceGuard {
  fn drop(&mut self) {
    unlock_file(&self._file);
    let _ = std_fs::remove_file(&self.path);
  }
}

pub struct CacheFileLock {
  path: PathBuf,
  _file: File,
}

impl Drop for CacheFileLock {
  fn drop(&mut self) {
    unlock_file(&self._file);
    let _ = std_fs::remove_file(&self.path);
  }
}

const LOCK_RECLAIM_GRACE: Duration = Duration::from_secs(2);
const CACHE_FILE_LOCK_STALE_AFTER: Duration = Duration::from_secs(600);
const INSTANCE_LOCK_STALE_AFTER: Duration = Duration::from_secs(30);

static CURRENT_INSTANCE_LOCK: OnceLock<PathBuf> = OnceLock::new();

pub fn register_instance(cache_dir: &Path) -> Result<CacheInstanceGuard> {
  let runtime_dir = cache_dir.join("runtime");
  std_fs::create_dir_all(&runtime_dir)
    .with_context(|| format!("failed to create {}", runtime_dir.display()))?;
  remove_stale_instance_locks_sync(&runtime_dir);
  let path = runtime_dir.join(format!(
    "instance-{}-{}.lock",
    std::process::id(),
    now_nanos()
  ));
  let file = create_locked_file_sync(&path)?;
  let _ = CURRENT_INSTANCE_LOCK.set(path.clone());
  Ok(CacheInstanceGuard { path, _file: file })
}

pub fn active_instance_count(cache_dir: &Path) -> usize {
  active_instance_count_sync(cache_dir)
}

pub async fn enforce_render_cache_limit(
  cache_dir: &Path,
  max_bytes: u64,
) -> Result<CacheCleanupReport> {
  if max_bytes == 0 {
    return Ok(CacheCleanupReport::default());
  }
  if active_instance_count(cache_dir) > 1 {
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
  if active_instance_count(cache_dir) > 1 {
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
  if active_instance_count(cache_dir) > 1 {
    return Err(anyhow!(
      "refusing to clear cache while another pdf-tui instance is running"
    ));
  }
  let _lock = acquire_cache_file_lock(&cache_dir.join("clear-cache")).await?;
  let targets = [
    cache_dir.join("pages"),
    cache_dir.join("render"),
    cache_dir.join("text"),
    cache_dir.join("search-highlight"),
    cache_dir.join("selection"),
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
  if active_instance_count(cache_dir) > 1 {
    return Ok(());
  }
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
        || path.starts_with(cache_dir.join("selection"))
    }
    Some("zst") => path.starts_with(cache_dir.join("text")),
    _ => false,
  }
}

pub async fn touch_cache_entry(cache_path: &Path) {
  let _ = write_bytes_atomic(&cache_used_path(cache_path), &[]).await;
}

pub fn touch_cache_entry_sync(cache_path: &Path) {
  let _ = write_bytes_atomic_sync(&cache_used_path(cache_path), &[]);
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

pub async fn acquire_cache_file_lock(cache_path: &Path) -> Result<CacheFileLock> {
  let lock_path = cache_lock_path(cache_path);
  if let Some(parent) = lock_path.parent() {
    fs::create_dir_all(parent)
      .await
      .with_context(|| format!("failed to create {}", parent.display()))?;
  }
  loop {
    match fs::OpenOptions::new()
      .read(true)
      .write(true)
      .create_new(true)
      .open(&lock_path)
      .await
    {
      Ok(file) => {
        let file = initialize_lock_file(file.into_std().await, &lock_path)?;
        return Ok(CacheFileLock {
          path: lock_path,
          _file: file,
        });
      }
      Err(error) if error.kind() == ErrorKind::AlreadyExists => {
        if reclaim_lock_file_sync(&lock_path, CACHE_FILE_LOCK_STALE_AFTER) {
          continue;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
      }
      Err(error) => {
        return Err(error)
          .with_context(|| format!("failed to create cache lock {}", lock_path.display()));
      }
    }
  }
}

pub fn acquire_cache_file_lock_sync(cache_path: &Path) -> Result<CacheFileLock> {
  let lock_path = cache_lock_path(cache_path);
  if let Some(parent) = lock_path.parent() {
    std_fs::create_dir_all(parent)
      .with_context(|| format!("failed to create {}", parent.display()))?;
  }
  loop {
    match OpenOptions::new()
      .read(true)
      .write(true)
      .create_new(true)
      .open(&lock_path)
    {
      Ok(file) => {
        let file = initialize_lock_file(file, &lock_path)?;
        return Ok(CacheFileLock {
          path: lock_path,
          _file: file,
        });
      }
      Err(error) if error.kind() == ErrorKind::AlreadyExists => {
        if reclaim_lock_file_sync(&lock_path, CACHE_FILE_LOCK_STALE_AFTER) {
          continue;
        }
        std::thread::sleep(Duration::from_millis(40));
      }
      Err(error) => {
        return Err(error)
          .with_context(|| format!("failed to create cache lock {}", lock_path.display()));
      }
    }
  }
}

pub async fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent)
      .await
      .with_context(|| format!("failed to create {}", parent.display()))?;
  }
  let temp_path = temp_sibling_path(path);
  fs::write(&temp_path, bytes)
    .await
    .with_context(|| format!("failed to write {}", temp_path.display()))?;
  persist_temp_file(&temp_path, path).await
}

pub fn write_bytes_atomic_sync(path: &Path, bytes: &[u8]) -> Result<()> {
  if let Some(parent) = path.parent() {
    std_fs::create_dir_all(parent)
      .with_context(|| format!("failed to create {}", parent.display()))?;
  }
  let temp_path = temp_sibling_path(path);
  std_fs::write(&temp_path, bytes)
    .with_context(|| format!("failed to write {}", temp_path.display()))?;
  persist_temp_file_sync(&temp_path, path)
}

pub async fn copy_file_atomic(source: &Path, path: &Path) -> Result<()> {
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent)
      .await
      .with_context(|| format!("failed to create {}", parent.display()))?;
  }
  let temp_path = temp_sibling_path(path);
  fs::copy(source, &temp_path).await.with_context(|| {
    format!(
      "failed to copy {} to {}",
      source.display(),
      temp_path.display()
    )
  })?;
  persist_temp_file(&temp_path, path).await
}

pub fn temp_sibling_path(path: &Path) -> PathBuf {
  let mut name = path
    .file_name()
    .map(|name| name.to_os_string())
    .unwrap_or_else(|| "cache".into());
  name.push(format!(".tmp-{}-{}", std::process::id(), now_nanos()));
  path.with_file_name(name)
}

pub fn persist_temp_file_sync(temp_path: &Path, output_path: &Path) -> Result<()> {
  replace_file_sync(temp_path, output_path)
}

async fn persist_temp_file(temp_path: &Path, output_path: &Path) -> Result<()> {
  let temp_path = temp_path.to_path_buf();
  let output_path = output_path.to_path_buf();
  tokio::task::spawn_blocking(move || persist_temp_file_sync(&temp_path, &output_path))
    .await
    .context("cache file replace task failed")?
}

#[cfg(unix)]
fn replace_file_sync(temp_path: &Path, output_path: &Path) -> Result<()> {
  std_fs::rename(temp_path, output_path).with_context(|| {
    format!(
      "failed to atomically replace {} with {}",
      output_path.display(),
      temp_path.display()
    )
  })
}

#[cfg(windows)]
fn replace_file_sync(temp_path: &Path, output_path: &Path) -> Result<()> {
  use std::os::windows::ffi::OsStrExt;
  use windows_sys::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
  };

  let source = temp_path
    .as_os_str()
    .encode_wide()
    .chain(std::iter::once(0))
    .collect::<Vec<_>>();
  let target = output_path
    .as_os_str()
    .encode_wide()
    .chain(std::iter::once(0))
    .collect::<Vec<_>>();
  let flags = MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH;
  let replaced = unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), flags) };
  if replaced != 0 {
    return Ok(());
  }

  let error = std::io::Error::last_os_error();
  let _ = std_fs::remove_file(temp_path);
  Err(error).with_context(|| {
    format!(
      "failed to atomically replace {} with {}",
      output_path.display(),
      temp_path.display()
    )
  })
}

#[cfg(not(any(unix, windows)))]
fn replace_file_sync(temp_path: &Path, output_path: &Path) -> Result<()> {
  std_fs::rename(temp_path, output_path).with_context(|| {
    format!(
      "failed to replace {} with {}",
      output_path.display(),
      temp_path.display()
    )
  })
}

fn cache_lock_path(cache_path: &Path) -> PathBuf {
  let mut name = cache_path
    .file_name()
    .map(|name| name.to_os_string())
    .unwrap_or_else(|| "cache".into());
  name.push(".lock");
  cache_path.with_file_name(name)
}

fn create_locked_file_sync(path: &Path) -> Result<File> {
  let file = OpenOptions::new()
    .read(true)
    .write(true)
    .create_new(true)
    .open(path)
    .with_context(|| format!("failed to create {}", path.display()))?;
  initialize_lock_file(file, path)
}

fn initialize_lock_file(mut file: File, path: &Path) -> Result<File> {
  if !try_lock_file(&file)? {
    return Err(anyhow!(
      "newly-created lock file is already locked: {}",
      path.display()
    ));
  }
  file
    .write_all(lock_file_body().as_bytes())
    .with_context(|| format!("failed to write {}", path.display()))?;
  Ok(file)
}

fn lock_file_body() -> String {
  format!(
    "pid={}\ncreated_nanos={}\n",
    std::process::id(),
    now_nanos()
  )
}

fn reclaim_lock_file_sync(path: &Path, stale_after: Duration) -> bool {
  if CURRENT_INSTANCE_LOCK
    .get()
    .is_some_and(|current| current == path)
  {
    return false;
  }
  let metadata = match std_fs::metadata(path) {
    Ok(metadata) => metadata,
    Err(error) if error.kind() == ErrorKind::NotFound => return true,
    Err(_) => return false,
  };
  let age = metadata
    .modified()
    .ok()
    .and_then(|time| time.elapsed().ok());
  if age.is_some_and(|age| age < LOCK_RECLAIM_GRACE) {
    return false;
  }

  let file = match OpenOptions::new().read(true).write(true).open(path) {
    Ok(file) => file,
    Err(error) if error.kind() == ErrorKind::NotFound => return true,
    Err(_) => return false,
  };
  let lock_available = match try_lock_file(&file) {
    Ok(lock_available) => lock_available,
    Err(_) => return false,
  };
  if !lock_available {
    return false;
  }

  let pid_alive = std_fs::read_to_string(path)
    .ok()
    .and_then(|body| lock_pid(&body))
    .is_some_and(process_alive);
  let stale = !pid_alive || age.is_none_or(|age| age > stale_after);
  unlock_file(&file);
  drop(file);
  if stale {
    let _ = std_fs::remove_file(path);
  }
  stale
}

fn active_instance_count_sync(cache_dir: &Path) -> usize {
  let runtime_dir = cache_dir.join("runtime");
  remove_stale_instance_locks_sync(&runtime_dir);
  let Ok(reader) = std_fs::read_dir(&runtime_dir) else {
    return 0;
  };
  reader
    .filter_map(Result::ok)
    .map(|entry| entry.path())
    .filter(|path| {
      path
        .extension()
        .is_some_and(|extension| extension == "lock")
    })
    .count()
}

fn remove_stale_instance_locks_sync(runtime_dir: &Path) {
  let Ok(reader) = std_fs::read_dir(runtime_dir) else {
    return;
  };
  for entry in reader.flatten() {
    let path = entry.path();
    if path
      .extension()
      .is_some_and(|extension| extension == "lock")
    {
      let _ = reclaim_lock_file_sync(&path, INSTANCE_LOCK_STALE_AFTER);
    }
  }
}

fn lock_pid(body: &str) -> Option<u32> {
  body
    .lines()
    .find_map(|line| line.strip_prefix("pid=")?.parse::<u32>().ok())
}

#[cfg(unix)]
fn try_lock_file(file: &File) -> Result<bool> {
  use std::os::fd::AsRawFd;

  let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
  if result == 0 {
    return Ok(true);
  }
  let error = std::io::Error::last_os_error();
  if error.kind() == ErrorKind::WouldBlock {
    return Ok(false);
  }
  Err(error).context("failed to lock cache lock file")
}

#[cfg(unix)]
fn unlock_file(file: &File) {
  use std::os::fd::AsRawFd;

  let _ = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
}

#[cfg(windows)]
fn try_lock_file(file: &File) -> Result<bool> {
  use std::os::windows::io::AsRawHandle;
  use windows_sys::Win32::{
    Foundation::{ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION},
    Storage::FileSystem::{LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx},
    System::IO::OVERLAPPED,
  };

  let mut overlapped = unsafe { std::mem::zeroed::<OVERLAPPED>() };
  let result = unsafe {
    LockFileEx(
      file.as_raw_handle(),
      LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
      0,
      1,
      0,
      &mut overlapped,
    )
  };
  if result != 0 {
    return Ok(true);
  }

  let error = std::io::Error::last_os_error();
  let raw_error = error.raw_os_error().map(|error| error as u32);
  if matches!(
    raw_error,
    Some(ERROR_LOCK_VIOLATION) | Some(ERROR_SHARING_VIOLATION)
  ) {
    return Ok(false);
  }
  Err(error).context("failed to lock cache lock file")
}

#[cfg(windows)]
fn unlock_file(file: &File) {
  use std::os::windows::io::AsRawHandle;
  use windows_sys::Win32::{Storage::FileSystem::UnlockFileEx, System::IO::OVERLAPPED};

  let mut overlapped = unsafe { std::mem::zeroed::<OVERLAPPED>() };
  let _ = unsafe { UnlockFileEx(file.as_raw_handle(), 0, 1, 0, &mut overlapped) };
}

#[cfg(not(any(unix, windows)))]
fn try_lock_file(_file: &File) -> Result<bool> {
  Ok(true)
}

#[cfg(not(any(unix, windows)))]
fn unlock_file(_file: &File) {}

fn process_alive(pid: u32) -> bool {
  if pid == std::process::id() {
    return true;
  }
  process_alive_platform(pid)
}

#[cfg(unix)]
fn process_alive_platform(pid: u32) -> bool {
  let pid: libc::pid_t = match pid.try_into() {
    Ok(pid) => pid,
    Err(_) => return false,
  };
  let result = unsafe { libc::kill(pid, 0) };
  if result == 0 {
    return true;
  }
  match std::io::Error::last_os_error().raw_os_error() {
    Some(error) if error == libc::EPERM => true,
    Some(error) if error == libc::ESRCH => false,
    _ => false,
  }
}

#[cfg(windows)]
fn process_alive_platform(pid: u32) -> bool {
  use windows_sys::Win32::{
    Foundation::{CloseHandle, STILL_ACTIVE},
    System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
  };

  let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
  if handle.is_null() {
    return false;
  }

  let mut exit_code = 0;
  let ok = unsafe { GetExitCodeProcess(handle, &mut exit_code) };
  unsafe {
    CloseHandle(handle);
  }
  ok != 0 && exit_code == STILL_ACTIVE as u32
}

#[cfg(not(any(unix, windows)))]
fn process_alive_platform(_pid: u32) -> bool {
  true
}

fn now_nanos() -> u128 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_nanos()
}
