//! Persistent per-document reading progress, stored as
//! `<cache_dir>/progress.toml`. Entries are keyed by document identity
//! (path, size, mtime) so edits to a PDF invalidate the saved position
//! instead of restoring a stale one. The store lives at the cache dir
//! root, outside the subdirectories wiped by `clear-cache`.

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

pub fn progress_file_path(cache_dir: &Path) -> PathBuf {
  cache_dir.join("progress.toml")
}

/// Saved progress for a document identity, or `None` when nothing matches
/// (never opened, or the file changed since the entry was written).
pub fn load_matching(
  cache_dir: &Path,
  path: &Path,
  size_bytes: u64,
  modified_nanos: u128,
) -> Option<f64> {
  let file = load(cache_dir);
  let entry = file.documents.iter().find(|entry| {
    entry.path == path.to_string_lossy()
      && entry.size_bytes == size_bytes
      && entry.modified_nanos == modified_nanos.to_string()
  })?;
  entry.progress.is_finite().then_some(entry.progress)
}

pub fn upsert(cache_dir: &Path, entry: ProgressEntry) -> Result<()> {
  let mut file = load(cache_dir);
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
  write_atomic_sync(&progress_file_path(cache_dir), encoded.as_bytes())
}

fn load(cache_dir: &Path) -> ProgressFile {
  let path = progress_file_path(cache_dir);
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

fn write_atomic_sync(path: &Path, bytes: &[u8]) -> Result<()> {
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
  }
  let temp_path = path.with_extension(format!("tmp-{}", std::process::id()));
  fs::write(&temp_path, bytes)
    .with_context(|| format!("failed to write {}", temp_path.display()))?;
  fs::rename(&temp_path, path)
    .with_context(|| format!("failed to rename into {}", path.display()))?;
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn tmp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdf-tui-progress-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
  }

  fn entry(path: &str, progress: f64, updated_secs: u64) -> ProgressEntry {
    ProgressEntry {
      path: path.into(),
      progress,
      size_bytes: 10,
      modified_nanos: "111".into(),
      page_count: 8,
      updated_secs,
    }
  }

  #[test]
  fn round_trip_matches_identity_and_invalidates_changes() {
    let dir = tmp_dir("round-trip");
    upsert(&dir, entry("/a.pdf", 3.5, 100)).unwrap();
    assert_eq!(load_matching(&dir, Path::new("/a.pdf"), 10, 111), Some(3.5));
    // A different document, size, or mtime must not match.
    assert_eq!(load_matching(&dir, Path::new("/b.pdf"), 10, 111), None);
    assert_eq!(load_matching(&dir, Path::new("/a.pdf"), 11, 111), None);
    assert_eq!(load_matching(&dir, Path::new("/a.pdf"), 10, 222), None);
    // Upserting replaces the stored position.
    upsert(&dir, entry("/a.pdf", 5.0, 200)).unwrap();
    assert_eq!(load_matching(&dir, Path::new("/a.pdf"), 10, 111), Some(5.0));
    // A corrupt store is ignored rather than fatal.
    fs::write(progress_file_path(&dir), "not toml {{{").unwrap();
    assert_eq!(load_matching(&dir, Path::new("/a.pdf"), 10, 111), None);
    let _ = fs::remove_dir_all(&dir);
  }

  #[test]
  fn upsert_keeps_only_recent_entries() {
    let dir = tmp_dir("trim");
    for index in 0..(MAX_ENTRIES + 5) {
      upsert(
        &dir,
        entry(&format!("/doc-{index:03}.pdf"), 1.0, index as u64 + 1),
      )
      .unwrap();
    }
    let raw = fs::read_to_string(progress_file_path(&dir)).unwrap();
    let file: ProgressFile = toml::from_str(&raw).unwrap();
    assert_eq!(file.documents.len(), MAX_ENTRIES);
    // Newest survived, oldest were dropped.
    assert!(
      file
        .documents
        .iter()
        .any(|entry| entry.path == "/doc-104.pdf")
    );
    assert!(
      !file
        .documents
        .iter()
        .any(|entry| entry.path == "/doc-000.pdf")
    );
    let _ = fs::remove_dir_all(&dir);
  }
}
