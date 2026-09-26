//! Building the embedded-text index: `pdftotext -tsv` output is parsed into
//! lines of positioned words and cached as zstd-compressed TOML under
//! `<cache>/text/`.

use std::{io::Cursor, path::Path};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{fs as async_fs, process::Command};

use crate::cache;

use super::{PdfSearchIndex, SearchLine, SearchRect, SearchWord, TextSpan};

/// On-disk cache wrapper. The layout is shared with older releases, so
/// fields must not be renamed or removed.
#[derive(Debug, Serialize, Deserialize)]
struct CachedSearchIndex {
  version: u8,
  source_path: String,
  source_size_bytes: u64,
  source_modified_nanos: String,
  pdftotext_bin: String,
  page_count: usize,
  index: PdfSearchIndex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LineKey {
  page_index: usize,
  par: usize,
  block: usize,
  line: usize,
}

pub async fn build_search_index(
  path: &Path,
  cache_dir: &Path,
  pdftotext_bin: &str,
  page_count: usize,
  source_size_bytes: u64,
  source_modified_nanos: u128,
) -> Result<PdfSearchIndex, String> {
  let cache_path = search_index_cache_path(
    cache_dir,
    path,
    pdftotext_bin,
    page_count,
    source_size_bytes,
    source_modified_nanos,
  );
  if let Ok(index) = read_cached_search_index(&cache_path).await {
    return Ok(index);
  }
  let _lock = cache::acquire_cache_file_lock(&cache_path)
    .await
    .map_err(|error| {
      format!(
        "failed to lock search cache {}: {error}",
        cache_path.display()
      )
    })?;
  if let Ok(index) = read_cached_search_index(&cache_path).await {
    return Ok(index);
  }

  let output = Command::new(pdftotext_bin)
    .arg("-tsv")
    .arg(path)
    .arg("-")
    .output()
    .await
    .map_err(|err| format!("failed to run {pdftotext_bin}; install poppler-utils: {err}"))?;
  if !output.status.success() {
    return Err(format!(
      "pdftotext -tsv failed: {}{}",
      String::from_utf8_lossy(&output.stderr).trim(),
      String::from_utf8_lossy(&output.stdout).trim()
    ));
  }
  let body = String::from_utf8(output.stdout)
    .map_err(|err| format!("pdftotext output is not UTF-8: {err}"))?;
  let index = parse_tsv_index(&body, page_count);
  let _ = write_cached_search_index(
    &cache_path,
    CachedSearchIndex {
      version: 1,
      source_path: path.to_string_lossy().into_owned(),
      source_size_bytes,
      source_modified_nanos: source_modified_nanos.to_string(),
      pdftotext_bin: pdftotext_bin.to_string(),
      page_count,
      index: index.clone(),
    },
  )
  .await;
  Ok(index)
}

pub(super) fn parse_tsv_index(body: &str, page_count: usize) -> PdfSearchIndex {
  let mut lines = Vec::new();
  let mut page_sizes = vec![(595.0, 842.0); page_count.max(1)];
  let mut current_key = None;
  let mut current_words = Vec::new();

  for raw in body.lines().skip(1) {
    let columns = raw.splitn(12, '\t').collect::<Vec<_>>();
    if columns.len() < 12 {
      continue;
    }
    let level = parse_usize(columns[0]).unwrap_or(0);
    let page_index = parse_usize(columns[1])
      .unwrap_or(1)
      .saturating_sub(1)
      .min(page_sizes.len().saturating_sub(1));
    let left = parse_f64(columns[6]).unwrap_or(0.0);
    let top = parse_f64(columns[7]).unwrap_or(0.0);
    let width = parse_f64(columns[8]).unwrap_or(0.0).max(0.0);
    let height = parse_f64(columns[9]).unwrap_or(0.0).max(0.0);
    let text = columns[11];

    if level == 1 && text == "###PAGE###" {
      page_sizes[page_index] = (width.max(1.0), height.max(1.0));
      continue;
    }
    if level != 5 || text.trim().is_empty() {
      continue;
    }

    let key = LineKey {
      page_index,
      par: parse_usize(columns[2]).unwrap_or(0),
      block: parse_usize(columns[3]).unwrap_or(0),
      line: parse_usize(columns[4]).unwrap_or(0),
    };
    if current_key.is_some_and(|current| current != key) {
      flush_line(
        &mut lines,
        current_key.take(),
        &mut current_words,
        &page_sizes,
      );
    }
    current_key = Some(key);
    current_words.push(SearchWord {
      text: text.to_string(),
      rect: SearchRect {
        x_min: left,
        y_min: top,
        x_max: left + width,
        y_max: top + height,
      },
    });
  }
  flush_line(&mut lines, current_key, &mut current_words, &page_sizes);
  PdfSearchIndex::new(lines)
}

fn flush_line(
  lines: &mut Vec<SearchLine>,
  key: Option<LineKey>,
  words: &mut Vec<SearchWord>,
  page_sizes: &[(f64, f64)],
) {
  let Some(key) = key else {
    return;
  };
  if words.is_empty() {
    return;
  }
  let mut display_text = String::new();
  let mut display_spans = Vec::with_capacity(words.len());
  let mut search_text = String::new();
  let mut search_spans = Vec::with_capacity(words.len());
  for (word_index, word) in words.iter().enumerate() {
    if !display_text.is_empty() {
      display_text.push(' ');
    }
    let display_start = display_text.len();
    display_text.push_str(&word.text);
    display_spans.push(TextSpan {
      word_index,
      start: display_start,
      end: display_text.len(),
    });

    let search_start = search_text.len();
    search_text.push_str(&word.text);
    search_spans.push(TextSpan {
      word_index,
      start: search_start,
      end: search_text.len(),
    });
  }
  let (page_width, page_height) = page_sizes
    .get(key.page_index)
    .copied()
    .unwrap_or((595.0, 842.0));
  lines.push(SearchLine {
    page_index: key.page_index,
    page_width,
    page_height,
    words: std::mem::take(words),
    display_text,
    display_spans,
    search_text,
    search_spans,
  });
}

fn search_index_cache_path(
  cache_dir: &Path,
  path: &Path,
  pdftotext_bin: &str,
  page_count: usize,
  source_size_bytes: u64,
  source_modified_nanos: u128,
) -> std::path::PathBuf {
  let mut hasher = Sha256::new();
  hasher.update(b"pdf-tui-search-index-v1");
  hasher.update(path.to_string_lossy().as_bytes());
  hasher.update(source_size_bytes.to_le_bytes());
  hasher.update(source_modified_nanos.to_le_bytes());
  hasher.update(page_count.to_le_bytes());
  hasher.update(pdftotext_bin.as_bytes());
  cache_dir
    .join("text")
    .join(format!("{}.toml.zst", hex::encode(hasher.finalize())))
}

async fn read_cached_search_index(path: &Path) -> Result<PdfSearchIndex, String> {
  let bytes = async_fs::read(path)
    .await
    .map_err(|error| format!("failed to read search cache {}: {error}", path.display()))?;
  let decoded = tokio::task::spawn_blocking(move || zstd::stream::decode_all(Cursor::new(bytes)))
    .await
    .map_err(|error| format!("search cache decode worker failed: {error}"))?
    .map_err(|error| format!("failed to decode search cache {}: {error}", path.display()))?;
  let decoded = String::from_utf8(decoded)
    .map_err(|error| format!("search cache {} is not UTF-8: {error}", path.display()))?;
  let cached: CachedSearchIndex = toml::from_str(&decoded)
    .map_err(|error| format!("failed to parse search cache {}: {error}", path.display()))?;
  if cached.version != 1 {
    return Err(format!(
      "unsupported search cache version {}",
      cached.version
    ));
  }
  cache::touch_cache_entry(path).await;
  Ok(cached.index)
}

async fn write_cached_search_index(path: &Path, cached: CachedSearchIndex) -> Result<(), String> {
  if let Some(parent) = path.parent() {
    async_fs::create_dir_all(parent)
      .await
      .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
  }
  let encoded = toml::to_string(&cached)
    .map_err(|error| format!("failed to encode search cache {}: {error}", path.display()))?;
  let compressed =
    tokio::task::spawn_blocking(move || zstd::stream::encode_all(Cursor::new(encoded), 3))
      .await
      .map_err(|error| format!("search cache compression worker failed: {error}"))?
      .map_err(|error| {
        format!(
          "failed to compress search cache {}: {error}",
          path.display()
        )
      })?;
  cache::write_bytes_atomic(path, &compressed)
    .await
    .map_err(|error| format!("failed to write search cache {}: {error}", path.display()))?;
  cache::touch_cache_entry(path).await;
  Ok(())
}

fn parse_usize(value: &str) -> Option<usize> {
  value.parse::<usize>().ok()
}

fn parse_f64(value: &str) -> Option<f64> {
  value.parse::<f64>().ok()
}
