use std::{fs, io::Cursor, path::Path, time::UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{fs as async_fs, process::Command};
use unicode_width::UnicodeWidthStr;

use crate::{cache, pdf::PageImage, selection::PdfSelection};

const MAX_SEARCH_RESULTS: usize = 2000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PdfSearchIndex {
  lines: Vec<SearchLine>,
}

#[derive(Debug, Clone)]
pub struct PdfSearchMatch {
  pub id: usize,
  pub page_index: usize,
  pub page_width: f64,
  pub page_height: f64,
  pub display_text: String,
  pub display_match_start: usize,
  pub display_match_end: usize,
  pub rect: SearchRect,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SearchRect {
  pub x_min: f64,
  pub y_min: f64,
  pub x_max: f64,
  pub y_max: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SearchLine {
  page_index: usize,
  page_width: f64,
  page_height: f64,
  words: Vec<SearchWord>,
  display_text: String,
  display_spans: Vec<TextSpan>,
  search_text: String,
  search_spans: Vec<TextSpan>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SearchWord {
  text: String,
  rect: SearchRect,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct TextSpan {
  word_index: usize,
  start: usize,
  end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LineKey {
  page_index: usize,
  par: usize,
  block: usize,
  line: usize,
}

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
  let index = parse_tsv_index(&body, page_count)?;
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

impl PdfSearchIndex {
  pub fn search(&self, query: &str) -> Vec<PdfSearchMatch> {
    let needle = normalize_search_text(query);
    if needle.is_empty() {
      return Vec::new();
    }
    let mut matches = Vec::new();
    for line in &self.lines {
      let haystack = normalize_search_text(&line.search_text);
      for (start, _) in haystack.match_indices(&needle) {
        let end = start.saturating_add(needle.len());
        let overlapping = line
          .search_spans
          .iter()
          .filter(|span| span.start < end && span.end > start)
          .copied()
          .collect::<Vec<_>>();
        if overlapping.is_empty() {
          continue;
        }
        let rect = overlapping
          .iter()
          .filter_map(|span| {
            let word = line.words.get(span.word_index)?;
            let local_start = start.max(span.start).saturating_sub(span.start);
            let local_end = end.min(span.end).saturating_sub(span.start);
            Some(partial_word_rect(word, local_start, local_end))
          })
          .reduce(union_rect);
        let Some(rect) = rect else {
          continue;
        };
        let display_start = overlapping
          .iter()
          .filter_map(|span| {
            let display_span = line.display_spans.get(span.word_index)?;
            Some(display_span.start + start.max(span.start).saturating_sub(span.start))
          })
          .min()
          .unwrap_or(0);
        let display_end = overlapping
          .iter()
          .filter_map(|span| {
            let display_span = line.display_spans.get(span.word_index)?;
            Some(display_span.start + end.min(span.end).saturating_sub(span.start))
          })
          .max()
          .unwrap_or(display_start);
        matches.push(PdfSearchMatch {
          id: matches.len(),
          page_index: line.page_index,
          page_width: line.page_width,
          page_height: line.page_height,
          display_text: line.display_text.clone(),
          display_match_start: display_start,
          display_match_end: display_end,
          rect,
        });
        if matches.len() >= MAX_SEARCH_RESULTS {
          return matches;
        }
      }
    }
    matches
  }

  pub fn text_in_selection(&self, selection: PdfSelection) -> String {
    let mut lines = Vec::new();
    for line in &self.lines {
      if line.page_index != selection.page_index {
        continue;
      }
      let rect = scale_selection_rect_to_line(selection, line);
      let words = line
        .words
        .iter()
        .filter(|word| rects_intersect(word.rect, rect))
        .map(|word| word.text.as_str())
        .collect::<Vec<_>>();
      if !words.is_empty() {
        lines.push(words.join(" "));
      }
    }
    lines.join("\n")
  }
}

fn scale_selection_rect_to_line(selection: PdfSelection, line: &SearchLine) -> SearchRect {
  let x_scale = line.page_width.max(1.0) / selection.page_width.max(1.0);
  let y_scale = line.page_height.max(1.0) / selection.page_height.max(1.0);
  SearchRect {
    x_min: selection.rect.x_min * x_scale,
    y_min: selection.rect.y_min * y_scale,
    x_max: selection.rect.x_max * x_scale,
    y_max: selection.rect.y_max * y_scale,
  }
}

pub fn highlighted_page_image(
  cache_dir: &Path,
  page: &PageImage,
  search_match: &PdfSearchMatch,
  max_bytes: u64,
) -> Result<PageImage, String> {
  highlighted_viewer_image(cache_dir, page, search_match, max_bytes)?
    .ok_or_else(|| "search match is outside the rendered page".to_string())
}

pub fn highlighted_viewer_image(
  cache_dir: &Path,
  page: &PageImage,
  search_match: &PdfSearchMatch,
  max_bytes: u64,
) -> Result<Option<PageImage>, String> {
  if page.page_index != search_match.page_index {
    return Ok(None);
  }
  if highlight_pixel_rect(page, search_match).is_none() {
    return Ok(None);
  }
  let dir = cache_dir.join("search-highlight");
  fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
  let cache_key = highlighted_cache_key(page, search_match);
  let path = dir.join(format!("{cache_key}.png"));
  if !path.exists() {
    let _lock = cache::acquire_cache_file_lock_sync(&path).map_err(|error| error.to_string())?;
    if path.exists() {
      cache::touch_cache_entry_sync(&path);
      return page_image_from_highlight_path(page, path);
    }
    write_highlighted_page(&path, page, search_match)?;
    let _ = cache::enforce_cache_target_limit_sync(cache_dir, &dir, max_bytes);
  }
  cache::touch_cache_entry_sync(&path);
  page_image_from_highlight_path(page, path)
}

fn page_image_from_highlight_path(
  page: &PageImage,
  path: std::path::PathBuf,
) -> Result<Option<PageImage>, String> {
  let metadata = fs::metadata(&path).map_err(|err| err.to_string())?;
  Ok(Some(PageImage {
    page_index: page.page_index,
    path,
    width: page.width,
    height: page.height,
    size_bytes: metadata.len(),
    modified_nanos: metadata
      .modified()
      .ok()
      .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
      .map(|duration| duration.as_nanos())
      .unwrap_or_default(),
    slice: page.slice.clone(),
  }))
}

fn parse_tsv_index(body: &str, page_count: usize) -> Result<PdfSearchIndex, String> {
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
  Ok(PdfSearchIndex { lines })
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
    let display_end = display_text.len();
    display_spans.push(TextSpan {
      word_index,
      start: display_start,
      end: display_end,
    });

    let search_start = search_text.len();
    search_text.push_str(&word.text);
    let search_end = search_text.len();
    search_spans.push(TextSpan {
      word_index,
      start: search_start,
      end: search_end,
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

fn write_highlighted_page(
  path: &Path,
  page: &PageImage,
  search_match: &PdfSearchMatch,
) -> Result<(), String> {
  let image = image::open(&page.path)
    .map_err(|err| format!("failed to open {}: {err}", page.path.display()))?;
  let mut image = image.to_rgba8();
  let Some((x0, y0, x1, y1)) = highlight_pixel_rect(page, search_match) else {
    return Ok(());
  };
  for y in y0..y1 {
    for x in x0..x1 {
      let pixel = image.get_pixel_mut(x, y);
      pixel.0[0] = 255u8.saturating_sub(pixel.0[0]);
      pixel.0[1] = 255u8.saturating_sub(pixel.0[1]);
      pixel.0[2] = 255u8.saturating_sub(pixel.0[2]);
    }
  }
  let temp_path = cache::temp_sibling_path(path);
  image
    .save(&temp_path)
    .map_err(|err| format!("failed to write {}: {err}", temp_path.display()))?;
  cache::persist_temp_file_sync(&temp_path, path)
    .map_err(|err| format!("failed to move highlighted cache {}: {err}", path.display()))
}

fn highlighted_cache_key(page: &PageImage, search_match: &PdfSearchMatch) -> String {
  let mut hasher = Sha256::new();
  hasher.update(b"pdf-tui-search-highlight-v1");
  hasher.update(page.path.to_string_lossy().as_bytes());
  hasher.update(page.size_bytes.to_le_bytes());
  hasher.update(page.modified_nanos.to_le_bytes());
  hasher.update(page.width.to_le_bytes());
  hasher.update(page.height.to_le_bytes());
  hasher.update(search_match.page_index.to_le_bytes());
  for value in [
    search_match.rect.x_min,
    search_match.rect.y_min,
    search_match.rect.x_max,
    search_match.rect.y_max,
    search_match.page_width,
    search_match.page_height,
  ] {
    hasher.update(value.to_le_bytes());
  }
  hex::encode(hasher.finalize())
}

fn highlight_pixel_rect(
  page: &PageImage,
  search_match: &PdfSearchMatch,
) -> Option<(u32, u32, u32, u32)> {
  let width = page.width.max(1);
  let height = page.height.max(1);
  let (full_width, full_height, slice_x, slice_y) = if let Some(slice) = &page.slice {
    (
      slice.full_pixel_width.max(1),
      slice.full_pixel_height.max(1),
      slice.slice_x,
      slice.slice_y,
    )
  } else {
    (width, height, 0, 0)
  };
  let x_scale = f64::from(full_width.max(1)) / search_match.page_width.max(1.0);
  let y_scale = f64::from(full_height.max(1)) / search_match.page_height.max(1.0);
  let full_x0 = (search_match.rect.x_min * x_scale).floor() as i64 - 2;
  let full_y0 = (search_match.rect.y_min * y_scale).floor() as i64 - 2;
  let full_x1 = (search_match.rect.x_max * x_scale).ceil() as i64 + 2;
  let full_y1 = (search_match.rect.y_max * y_scale).ceil() as i64 + 2;
  let x0 = full_x0 - i64::from(slice_x);
  let y0 = full_y0 - i64::from(slice_y);
  let x1 = full_x1 - i64::from(slice_x);
  let y1 = full_y1 - i64::from(slice_y);
  if x1 <= 0 || y1 <= 0 || x0 >= i64::from(width) || y0 >= i64::from(height) {
    return None;
  }
  let x0 = x0.clamp(0, i64::from(width.saturating_sub(1))) as u32;
  let y0 = y0.clamp(0, i64::from(height.saturating_sub(1))) as u32;
  let x1 = x1.clamp(i64::from(x0.saturating_add(1)), i64::from(width)) as u32;
  let y1 = y1.clamp(i64::from(y0.saturating_add(1)), i64::from(height)) as u32;
  Some((x0, y0, x1, y1))
}

fn normalize_search_text(value: &str) -> String {
  value
    .chars()
    .filter(|ch| !ch.is_whitespace())
    .map(|ch| ch.to_ascii_lowercase())
    .collect()
}

fn union_rect(left: SearchRect, right: SearchRect) -> SearchRect {
  SearchRect {
    x_min: left.x_min.min(right.x_min),
    y_min: left.y_min.min(right.y_min),
    x_max: left.x_max.max(right.x_max),
    y_max: left.y_max.max(right.y_max),
  }
}

fn rects_intersect(a: SearchRect, b: SearchRect) -> bool {
  a.x_min < b.x_max && b.x_min < a.x_max && a.y_min < b.y_max && b.y_min < a.y_max
}

fn partial_word_rect(word: &SearchWord, local_start: usize, local_end: usize) -> SearchRect {
  let local_start = local_start.min(word.text.len());
  let local_end = local_end.min(word.text.len()).max(local_start);
  let total_width = UnicodeWidthStr::width(word.text.as_str()).max(1) as f64;
  let before_width = word
    .text
    .get(..local_start)
    .map(UnicodeWidthStr::width)
    .unwrap_or(0) as f64;
  let matched_width = word
    .text
    .get(local_start..local_end)
    .map(UnicodeWidthStr::width)
    .unwrap_or_else(|| UnicodeWidthStr::width(word.text.as_str()))
    .max(1) as f64;
  let word_width = (word.rect.x_max - word.rect.x_min).max(0.0);
  let x_min = word.rect.x_min + word_width * (before_width / total_width);
  let x_max = word.rect.x_min + word_width * ((before_width + matched_width) / total_width);
  SearchRect {
    x_min: x_min.min(word.rect.x_max),
    y_min: word.rect.y_min,
    x_max: x_max.clamp(x_min, word.rect.x_max),
    y_max: word.rect.y_max,
  }
}

fn parse_usize(value: &str) -> Option<usize> {
  value.parse::<usize>().ok()
}

fn parse_f64(value: &str) -> Option<f64> {
  value.parse::<f64>().ok()
}
