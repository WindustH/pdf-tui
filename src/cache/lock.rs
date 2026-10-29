//! Cross-process locking: per-entry cache locks and the instance registry
//! used to tell whether other pdf-tui processes share the cache.
//!
//! Entry locks retain a stable `.lock` inode and use an OS file lock
//! (`flock` on Unix, `LockFileEx` on Windows). The kernel releases ownership
//! when a process dies. Instance registry files use unique names and may be removed.

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
    // Acquirers use create_new, so keep the OS lock held until this name is
    // removed. Never unlink after another owner could reclaim the path.
    let _ = std_fs::remove_file(&self.path);
    unlock_file(&self._file);
  }
}

pub struct CacheFileLock {
  _file: File,
}

impl Drop for CacheFileLock {
  fn drop(&mut self) {
    // Keep the pathname stable: a waiter may already have opened this inode.
    unlock_file(&self._file);
  }
}

const LOCK_RECLAIM_GRACE: Duration = Duration::from_secs(2);
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

/// One attempt at taking a persistent entry lock; `None` while held.
fn try_create_cache_lock(lock_path: &Path) -> Result<Option<CacheFileLock>> {
  let file = OpenOptions::new()
    .read(true)
    .write(true)
    .create(true)
    .truncate(false)
    .open(lock_path)
    .with_context(|| format!("failed to open cache lock {}", lock_path.display()))?;
  if try_lock_file(&file)? {
    Ok(Some(CacheFileLock { _file: file }))
  } else {
    Ok(None)
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
  let removed = stale && std_fs::remove_file(path).is_ok();
  unlock_file(&file);
  removed
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

pub fn acquire_document_lock(path: &Path) -> Result<CacheFileLock> {
  use sha2::{Digest, Sha256};
  let path = path.canonicalize().unwrap_or_else(|_| path.to_owned());
  let key = hex::encode(Sha256::digest(path.as_os_str().as_encoded_bytes()));
  let root = framework_tui::paths::cache_dir().join("pdf-tui/document-locks");
  std_fs::create_dir_all(&root)?;
  acquire_cache_file_lock_sync(&root.join(key))
}
