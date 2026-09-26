//! Cross-process locking: per-entry cache locks and the instance registry
//! used to tell whether other pdf-tui processes share the cache.
//!
//! A lock is a `.lock` file created with `create_new` and held with an OS
//! file lock (`flock` on Unix, `LockFileEx` on Windows). The kernel drops
//! the OS lock when a process dies, so leftover lock files can be reclaimed.

use std::{
  fs::{self as std_fs, File, OpenOptions},
  io::{ErrorKind, Write},
  path::{Path, PathBuf},
  sync::OnceLock,
  time::Duration,
};

use anyhow::{Context, Result, anyhow};
use tokio::fs;

use super::now_nanos;

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
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(40);

static CURRENT_INSTANCE_LOCK: OnceLock<PathBuf> = OnceLock::new();

/// Registers this process in `<cache>/runtime/` for as long as the guard
/// lives.
pub fn register_instance(cache_dir: &Path) -> Result<CacheInstanceGuard> {
  let runtime_dir = cache_dir.join("runtime");
  std_fs::create_dir_all(&runtime_dir)
    .with_context(|| format!("failed to create {}", runtime_dir.display()))?;
  remove_stale_instance_locks(&runtime_dir);
  let path = runtime_dir.join(format!(
    "instance-{}-{}.lock",
    std::process::id(),
    now_nanos()
  ));
  let file = create_locked_file(&path)?;
  let _ = CURRENT_INSTANCE_LOCK.set(path.clone());
  Ok(CacheInstanceGuard { path, _file: file })
}

/// Number of live pdf-tui processes using `cache_dir`, this one included.
pub fn active_instance_count(cache_dir: &Path) -> usize {
  let runtime_dir = cache_dir.join("runtime");
  remove_stale_instance_locks(&runtime_dir);
  let Ok(reader) = std_fs::read_dir(&runtime_dir) else {
    return 0;
  };
  reader
    .filter_map(Result::ok)
    .filter(|entry| is_lock_file(&entry.path()))
    .count()
}

/// Waits for exclusive ownership of the lock guarding `cache_path`.
pub async fn acquire_cache_file_lock(cache_path: &Path) -> Result<CacheFileLock> {
  let lock_path = cache_lock_path(cache_path);
  if let Some(parent) = lock_path.parent() {
    fs::create_dir_all(parent)
      .await
      .with_context(|| format!("failed to create {}", parent.display()))?;
  }
  loop {
    match try_create_cache_lock(&lock_path)? {
      Some(lock) => return Ok(lock),
      None => tokio::time::sleep(LOCK_RETRY_DELAY).await,
    }
  }
}

/// Blocking variant of [`acquire_cache_file_lock`].
pub fn acquire_cache_file_lock_sync(cache_path: &Path) -> Result<CacheFileLock> {
  let lock_path = cache_lock_path(cache_path);
  if let Some(parent) = lock_path.parent() {
    std_fs::create_dir_all(parent)
      .with_context(|| format!("failed to create {}", parent.display()))?;
  }
  loop {
    match try_create_cache_lock(&lock_path)? {
      Some(lock) => return Ok(lock),
      None => std::thread::sleep(LOCK_RETRY_DELAY),
    }
  }
}

/// One attempt at taking the lock: `None` while another live holder has
/// it; a stale leftover is reclaimed and retried immediately.
fn try_create_cache_lock(lock_path: &Path) -> Result<Option<CacheFileLock>> {
  loop {
    match OpenOptions::new()
      .read(true)
      .write(true)
      .create_new(true)
      .open(lock_path)
    {
      Ok(file) => {
        let file = initialize_lock_file(file, lock_path)?;
        return Ok(Some(CacheFileLock {
          path: lock_path.to_path_buf(),
          _file: file,
        }));
      }
      Err(error) if error.kind() == ErrorKind::AlreadyExists => {
        if !reclaim_lock_file(lock_path, CACHE_FILE_LOCK_STALE_AFTER) {
          return Ok(None);
        }
      }
      Err(error) => {
        return Err(error)
          .with_context(|| format!("failed to create cache lock {}", lock_path.display()));
      }
    }
  }
}

fn cache_lock_path(cache_path: &Path) -> PathBuf {
  let mut name = cache_path
    .file_name()
    .map(|name| name.to_os_string())
    .unwrap_or_else(|| "cache".into());
  name.push(".lock");
  cache_path.with_file_name(name)
}

fn is_lock_file(path: &Path) -> bool {
  path
    .extension()
    .is_some_and(|extension| extension == "lock")
}

fn create_locked_file(path: &Path) -> Result<File> {
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
    .write_all(
      format!(
        "pid={}\ncreated_nanos={}\n",
        std::process::id(),
        now_nanos()
      )
      .as_bytes(),
    )
    .with_context(|| format!("failed to write {}", path.display()))?;
  Ok(file)
}

/// Removes the lock file at `path` when nobody holds its OS lock and its
/// owner is gone or it is older than `stale_after`. Returns whether the
/// path is free now.
fn reclaim_lock_file(path: &Path, stale_after: Duration) -> bool {
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
  if !try_lock_file(&file).unwrap_or(false) {
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

fn remove_stale_instance_locks(runtime_dir: &Path) {
  let Ok(reader) = std_fs::read_dir(runtime_dir) else {
    return;
  };
  for entry in reader.flatten() {
    let path = entry.path();
    if is_lock_file(&path) {
      let _ = reclaim_lock_file(&path, INSTANCE_LOCK_STALE_AFTER);
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
  std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
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

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn cache_lock_is_exclusive_until_dropped() {
    let dir = std::env::temp_dir().join(format!("pdf-tui-lock-test-{}", std::process::id()));
    let _ = std_fs::remove_dir_all(&dir);
    std_fs::create_dir_all(&dir).unwrap();
    let entry = dir.join("entry.png");
    let lock = acquire_cache_file_lock_sync(&entry).unwrap();
    // A held lock is neither re-acquirable nor reclaimable.
    assert!(
      try_create_cache_lock(&cache_lock_path(&entry))
        .unwrap()
        .is_none()
    );
    drop(lock);
    assert!(!cache_lock_path(&entry).exists());
    assert!(
      try_create_cache_lock(&cache_lock_path(&entry))
        .unwrap()
        .is_some()
    );
    let _ = std_fs::remove_dir_all(&dir);
  }
}
