//! Embedded-text search: the positioned-word index built from
//! `pdftotext -tsv`, query matching, and highlighted preview images.

mod highlight;
mod index;

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthStr;

use crate::selection::PdfSelection;

pub use highlight::{highlight_applies, highlighted_viewer_image};
pub use index::build_search_index;

const MAX_SEARCH_RESULTS: usize = 2000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PdfSearchIndex {
  lines: Vec<SearchLine>,
  /// Normalized per-line haystacks, built on the first query. Not part of
  /// the cache format.
  #[serde(skip)]
  normalized: OnceLock<Vec<NormalizedLine>>,
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

/// One text line of the index. `search_text`/`search_spans` are kept only
/// for cache compatibility with older releases; matching works on the
/// normalized form built from `words`.
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

/// A line with whitespace removed and ASCII lowercased, plus the byte span
/// each word occupies in that normalized text.
#[derive(Debug, Clone)]
struct NormalizedLine {
  text: String,
  spans: Vec<TextSpan>,
}

impl PdfSearchIndex {
  fn new(lines: Vec<SearchLine>) -> Self {
    Self {
      lines,
      normalized: OnceLock::new(),
    }
  }

  pub fn search(&self, query: &str) -> Vec<PdfSearchMatch> {
    let needle = normalize_search_text(query);
    if needle.is_empty() {
      return Vec::new();
    }
    let normalized = self
      .normalized
      .get_or_init(|| self.lines.iter().map(normalize_line).collect());
    let mut matches = Vec::new();
    for (line, haystack) in self.lines.iter().zip(normalized) {
      for (start, _) in haystack.text.match_indices(&needle) {
        let end = start + needle.len();
        if let Some(found) = line_match(line, haystack, start, end, matches.len()) {
          matches.push(found);
          if matches.len() >= MAX_SEARCH_RESULTS {
            return matches;
          }
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

/// Builds the match for normalized byte range `start..end` of one line:
/// maps it back to the matched part of each overlapping word, unions their
/// rectangles, and finds the matched range inside `display_text`.
fn line_match(
  line: &SearchLine,
  haystack: &NormalizedLine,
  start: usize,
  end: usize,
  id: usize,
) -> Option<PdfSearchMatch> {
  let mut rect: Option<SearchRect> = None;
  let mut display_range: Option<(usize, usize)> = None;
  for span in haystack
    .spans
    .iter()
    .filter(|span| span.start < end && span.end > start)
  {
    let Some(word) = line.words.get(span.word_index) else {
      continue;
    };
    let (raw_start, raw_end) = raw_word_range(
      &word.text,
      start.saturating_sub(span.start),
      end.min(span.end) - span.start,
    );
    let word_rect = partial_word_rect(word, raw_start, raw_end);
    rect = Some(rect.map_or(word_rect, |rect| union_rect(rect, word_rect)));
    if let Some(display) = line.display_spans.get(span.word_index) {
      let range = (display.start + raw_start, display.start + raw_end);
      display_range =
        Some(display_range.map_or(range, |(lo, hi)| (lo.min(range.0), hi.max(range.1))));
    }
  }
  let (display_match_start, display_match_end) = display_range.unwrap_or((0, 0));
  Some(PdfSearchMatch {
    id,
    page_index: line.page_index,
    page_width: line.page_width,
    page_height: line.page_height,
    display_text: line.display_text.clone(),
    display_match_start,
    display_match_end,
    rect: rect?,
  })
}

fn normalize_line(line: &SearchLine) -> NormalizedLine {
  let mut text = String::new();
  let mut spans = Vec::with_capacity(line.words.len());
  for (word_index, word) in line.words.iter().enumerate() {
    let start = text.len();
    text.extend(normalized_chars(&word.text));
    spans.push(TextSpan {
      word_index,
      start,
      end: text.len(),
    });
  }
  NormalizedLine { text, spans }
}

fn normalize_search_text(value: &str) -> String {
  normalized_chars(value).collect()
}

/// Matching ignores whitespace and ASCII case. Lowercasing ASCII keeps byte
/// lengths, so normalized offsets map back to source characters one-to-one.
fn normalized_chars(value: &str) -> impl Iterator<Item = char> + '_ {
  value
    .chars()
    .filter(|ch| !ch.is_whitespace())
    .map(|ch| ch.to_ascii_lowercase())
}

/// Converts the normalized byte range `start..end` within one word into the
/// byte range of the same characters in the word's original text, skipping
/// whitespace the normalization dropped. Both ends are char boundaries.
fn raw_word_range(word: &str, start: usize, end: usize) -> (usize, usize) {
  let mut normalized = 0;
  let mut range: Option<(usize, usize)> = None;
  for (index, ch) in word.char_indices() {
    if ch.is_whitespace() {
      continue;
    }
    let char_start = normalized;
    normalized += ch.len_utf8();
    if char_start >= start && normalized <= end {
      let char_end = index + ch.len_utf8();
      range = Some(range.map_or((index, char_end), |(lo, _)| (lo, char_end)));
    }
  }
  range.unwrap_or((word.len(), word.len()))
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

/// Horizontal sub-rectangle of `word` covering bytes `local_start..local_end`,
/// estimated from display widths since pdftotext only reports word boxes.
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

#[cfg(test)]
mod tests {
  use super::*;

  const HEADER: &str = "level\tpage_num\tpar_num\tblock_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n";

  fn tsv(words: &[&str]) -> String {
    let mut body = String::from(HEADER);
    body.push_str("1\t1\t0\t0\t0\t0\t0\t0\t600\t800\t-1\t###PAGE###\n");
    for (index, word) in words.iter().enumerate() {
      let left = 10 + index * 100;
      body.push_str(&format!(
        "5\t1\t0\t0\t0\t{index}\t{left}\t20\t90\t10\t100\t{word}\n"
      ));
    }
    body
  }

  fn matched_display(found: &PdfSearchMatch) -> &str {
    &found.display_text[found.display_match_start..found.display_match_end]
  }

  #[test]
  fn match_offsets_skip_whitespace_inside_words() {
    // pdftotext can report words containing ideographic or no-break spaces.
    // The normalized haystack drops them, so offsets must be mapped back
    // per character instead of reusing raw byte positions.
    let index = index::parse_tsv_index(&tsv(&["ab\u{3000}c\u{3000}1", "xyz"]), 1);
    let found = index.search("1");
    assert_eq!(found.len(), 1);
    assert_eq!(matched_display(&found[0]), "1");
    let found = index.search("c1x");
    assert_eq!(found.len(), 1);
    assert_eq!(matched_display(&found[0]), "c\u{3000}1 x");

    let index = index::parse_tsv_index(&tsv(&["\u{3000}\u{e9}", "b"]), 1);
    let found = index.search("b");
    assert_eq!(found.len(), 1);
    assert_eq!(matched_display(&found[0]), "b");
    // The match rectangle lies in the second word's box.
    assert!(found[0].rect.x_min >= 110.0);
  }

  #[test]
  fn search_is_case_and_space_insensitive() {
    let index = index::parse_tsv_index(&tsv(&["Hello", "World"]), 1);
    let found = index.search("o w");
    assert_eq!(found.len(), 1);
    assert_eq!(matched_display(&found[0]), "o W");
    assert!(index.search("   ").is_empty());
  }
}
