//! Persistent per-document reading progress, stored as
//! `<state_dir>/progress.toml`. Entries are keyed by document identity
//! (path, size, mtime) so edits to a PDF invalidate the saved position
//! instead of restoring a stale one. The store lives in the state dir, not
//! the cache, so clearing the cache never loses reading positions.

use std::{
  fs,
  path::{Path, PathBuf},
  time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::warn;

/// Cap on remembered documents; least recently updated entries are
/// dropped first so the file stays small.
const MAX_ENTRIES: usize = 100;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgressEntry {
  pub path: String,
  pub progress: f64,
  pub size_bytes: u64,
  /// Stored as a string: TOML integers cannot hold u128 nanos.
  pub modified_nanos: String,
  pub page_count: usize,
  pub updated_secs: u64,
}

impl ProgressEntry {
  pub fn new(
    path: &Path,
    progress: f64,
    size_bytes: u64,
    modified_nanos: u128,
    page_count: usize,
  ) -> Self {
    Self {
      path: path.to_string_lossy().into_owned(),
      progress,
      size_bytes,
      modified_nanos: modified_nanos.to_string(),
      page_count,
      updated_secs: SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default(),
    }
  }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ProgressFile {
  #[serde(default)]
  documents: Vec<ProgressEntry>,
}

const FILE_NAME: &str = "progress.toml";

pub fn progress_file_path(state_dir: &Path) -> PathBuf {
  state_dir.join(FILE_NAME)
}

/// The store used to live in the cache dir. Move it to the state dir unless
/// the state dir already has one; copy-then-remove also works across file
/// systems.
pub fn migrate_from_cache_dir(cache_dir: &Path, state_dir: &Path) {
  let from = cache_dir.join(FILE_NAME);
  let to = progress_file_path(state_dir);
  if !from.is_file() || to.exists() {
    return;
  }
  let moved = fs::create_dir_all(state_dir).and_then(|()| fs::copy(&from, &to));
  match moved {
    Ok(_) => {
      let _ = fs::remove_file(&from);
    }
    Err(error) => warn!(
      from = %from.display(),
      to = %to.display(),
      %error,
      "could not move the progress store out of the cache"
    ),
  }
}

/// Saved progress for a document identity, or `None` when nothing matches
/// (never opened, or the file changed since the entry was written).
pub fn load_matching(
  state_dir: &Path,
  path: &Path,
  size_bytes: u64,
  modified_nanos: u128,
) -> Option<f64> {
  let file = load(state_dir);
  let entry = file.documents.iter().find(|entry| {
    entry.path == path.to_string_lossy()
      && entry.size_bytes == size_bytes
      && entry.modified_nanos == modified_nanos.to_string()
  })?;
  entry.progress.is_finite().then_some(entry.progress)
}

/// Replaces the entry for `entry.path`. The read-modify-write runs under a
/// file lock so two instances closing at once keep both updates.
pub fn upsert(state_dir: &Path, entry: ProgressEntry) -> Result<()> {
  let path = progress_file_path(state_dir);
  let _lock = crate::cache::acquire_cache_file_lock_sync(&path)?;
  let mut file = load(state_dir);
  file
    .documents
    .retain(|existing| existing.path != entry.path);
  file.documents.push(entry);
  file.documents.sort_by(|a, b| {
    b.updated_secs
      .cmp(&a.updated_secs)
      .then(a.path.cmp(&b.path))
  });
  file.documents.truncate(MAX_ENTRIES);
  let encoded = toml::to_string_pretty(&file).context("failed to encode progress store")?;
  crate::cache::write_bytes_atomic_sync(&path, encoded.as_bytes())
}

fn load(state_dir: &Path) -> ProgressFile {
  let path = progress_file_path(state_dir);
  match fs::read_to_string(&path) {
    Ok(raw) => toml::from_str(&raw).unwrap_or_else(|error| {
      warn!(path = %path.display(), %error, "ignoring unreadable progress store");
      ProgressFile::default()
    }),
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => ProgressFile::default(),
    Err(error) => {
      warn!(path = %path.display(), %error, "ignoring unreadable progress store");
      ProgressFile::default()
    }
  }
}
