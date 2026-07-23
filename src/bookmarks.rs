use std::{
  fs,
  path::{Path, PathBuf},
  process::Command,
  time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfBookmark {
  pub id: usize,
  pub title: String,
  pub level: u16,
  pub page_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookmarkEdit {
  pub bookmarks: Vec<PdfBookmark>,
  pub original_count: usize,
}

impl BookmarkEdit {
  pub fn new_count(&self) -> usize {
    self.bookmarks.len()
  }
}

#[derive(Debug, Default)]
struct RawBookmark {
  title: Option<String>,
  level: Option<u16>,
  page_number: Option<usize>,
}

pub fn read_pdf_bookmarks(
  path: &Path,
  pdftk_bin: &str,
  page_count: usize,
) -> Result<Vec<PdfBookmark>, String> {
  let data = dump_pdf_data(path, pdftk_bin)?;
  Ok(parse_pdftk_bookmarks(&data, page_count))
}

pub fn bookmarks_edit_draft(path: &Path, bookmarks: &[PdfBookmark], page_count: usize) -> String {
  let mut out = String::new();
  out.push_str("# Edit PDF bookmarks. Save and exit to continue.\n");
  out.push_str("# pdf-tui will ask for confirmation before writing changes.\n");
  out.push_str("# Delete a [[bookmark]] block to remove it; add a block to create one.\n");
  out.push_str("# level is 1-based tree depth; page is 1-based PDF page number.\n");
  out.push_str(&format!("# file = {:?}\n", path.display().to_string()));
  out.push_str(&format!("# pages = {page_count}\n\n"));
  for bookmark in bookmarks {
    out.push_str("[[bookmark]]\n");
    out.push_str(&format!("level = {}\n", bookmark.level.max(1)));
    out.push_str(&format!("page = {}\n", bookmark.page_index + 1));
    out.push_str(&format!("title = {}\n\n", toml_string(&bookmark.title)));
  }
  out
}

pub fn bookmark_changes_from_edit(
  original: &[PdfBookmark],
  edited: &str,
  page_count: usize,
) -> Result<Option<BookmarkEdit>, String> {
  let value = toml::from_str::<toml::Value>(edited)
    .map_err(|err| format!("bookmark draft is not valid TOML: {err}"))?;
  let bookmarks = match value.get("bookmark") {
    Some(toml::Value::Array(items)) => parse_bookmark_array(items, page_count)?,
    Some(_) => return Err("bookmark draft [[bookmark]] entries must be tables".to_string()),
    None => Vec::new(),
  };
  if bookmark_signature(original) == bookmark_signature(&bookmarks) {
    return Ok(None);
  }
  Ok(Some(BookmarkEdit {
    bookmarks,
    original_count: original.len(),
  }))
}

pub fn write_pdf_bookmarks_with_pdftk(
  path: &Path,
  pdftk_bin: &str,
  cache_dir: &Path,
  bookmarks: &[PdfBookmark],
) -> Result<(), String> {
  let _lock =
    crate::cache::acquire_cache_file_lock_sync(path).map_err(|error| error.to_string())?;
  let original_data = dump_pdf_data(path, pdftk_bin)?;
  let update_info = replace_bookmarks_in_pdftk_data(&original_data, bookmarks);
  let work_dir = cache_dir.join("bookmarks");
  fs::create_dir_all(&work_dir).map_err(|err| err.to_string())?;
  let unique = temp_unique();
  let info_path = work_dir.join(format!("bookmarks-{unique}.info"));
  fs::write(&info_path, update_info).map_err(|err| err.to_string())?;
  let temp_pdf = temp_pdf_path(path, unique);
  let _ = fs::remove_file(&temp_pdf);
  let output = Command::new(pdftk_bin)
    .arg(path)
    .arg("update_info_utf8")
    .arg(&info_path)
    .arg("output")
    .arg(&temp_pdf)
    .arg("dont_ask")
    .output()
    .map_err(|err| format!("failed to run {pdftk_bin}; install pdftk or put it in PATH: {err}"))?;
  let _ = fs::remove_file(&info_path);
  if !output.status.success() {
    let _ = fs::remove_file(&temp_pdf);
    return Err(format!(
      "pdftk update_info_utf8 failed: {}{}",
      String::from_utf8_lossy(&output.stderr).trim(),
      String::from_utf8_lossy(&output.stdout).trim()
    ));
  }
  fs::metadata(&temp_pdf)
    .map_err(|err| format!("pdftk did not create {}: {err}", temp_pdf.display()))?;
  fs::rename(&temp_pdf, path).map_err(|err| {
    format!(
      "failed to replace {} with edited PDF: {err}",
      path.display()
    )
  })
}

fn dump_pdf_data(path: &Path, pdftk_bin: &str) -> Result<String, String> {
  let output = Command::new(pdftk_bin)
    .arg(path)
    .arg("dump_data_utf8")
    .output()
    .map_err(|err| format!("failed to run {pdftk_bin}; install pdftk or put it in PATH: {err}"))?;
  if !output.status.success() {
    return Err(format!(
      "pdftk dump_data_utf8 failed: {}{}",
      String::from_utf8_lossy(&output.stderr).trim(),
      String::from_utf8_lossy(&output.stdout).trim()
    ));
  }
  String::from_utf8(output.stdout).map_err(|err| format!("pdftk output is not UTF-8: {err}"))
}

fn parse_pdftk_bookmarks(data: &str, page_count: usize) -> Vec<PdfBookmark> {
  let mut bookmarks = Vec::new();
  let mut current: Option<RawBookmark> = None;
  for line in data.lines() {
    if line == "BookmarkBegin" {
      finish_raw_bookmark(&mut bookmarks, current.take(), page_count);
      current = Some(RawBookmark::default());
      continue;
    }
    let Some(raw) = current.as_mut() else {
      continue;
    };
    if let Some(value) = line.strip_prefix("BookmarkTitle:") {
      raw.title = Some(value.trim_start().to_string());
    } else if let Some(value) = line.strip_prefix("BookmarkLevel:") {
      raw.level = value.trim().parse::<u16>().ok();
    } else if let Some(value) = line.strip_prefix("BookmarkPageNumber:") {
      raw.page_number = value.trim().parse::<usize>().ok();
    } else if line.ends_with("Begin") {
      finish_raw_bookmark(&mut bookmarks, current.take(), page_count);
    }
  }
  finish_raw_bookmark(&mut bookmarks, current, page_count);
  bookmarks
}

fn finish_raw_bookmark(
  bookmarks: &mut Vec<PdfBookmark>,
  raw: Option<RawBookmark>,
  page_count: usize,
) {
  let Some(raw) = raw else {
    return;
  };
  let title = raw.title.unwrap_or_default();
  if title.trim().is_empty() {
    return;
  }
  let page_number = raw.page_number.unwrap_or(1).max(1);
  let page_index = page_number
    .saturating_sub(1)
    .min(page_count.saturating_sub(1));
  bookmarks.push(PdfBookmark {
    id: bookmarks.len(),
    title,
    level: raw.level.unwrap_or(1).max(1),
    page_index,
  });
}

fn parse_bookmark_array(
  items: &[toml::Value],
  page_count: usize,
) -> Result<Vec<PdfBookmark>, String> {
  let mut bookmarks = Vec::with_capacity(items.len());
  for item in items {
    let Some(table) = item.as_table() else {
      return Err("[[bookmark]] entries must be tables".to_string());
    };
    let level = table
      .get("level")
      .and_then(toml::Value::as_integer)
      .ok_or_else(|| "[[bookmark]] level must be an integer".to_string())?;
    let page = table
      .get("page")
      .and_then(toml::Value::as_integer)
      .ok_or_else(|| "[[bookmark]] page must be an integer".to_string())?;
    let title = table
      .get("title")
      .and_then(toml::Value::as_str)
      .ok_or_else(|| "[[bookmark]] title must be a string".to_string())?
      .trim()
      .to_string();
    if level < 1 {
      return Err("[[bookmark]] level must be >= 1".to_string());
    }
    if page < 1 || page as usize > page_count.max(1) {
      return Err(format!(
        "[[bookmark]] page must be between 1 and {}",
        page_count.max(1)
      ));
    }
    if title.is_empty() {
      return Err("[[bookmark]] title must not be empty".to_string());
    }
    bookmarks.push(PdfBookmark {
      id: bookmarks.len(),
      title,
      level: level.min(i64::from(u16::MAX)) as u16,
      page_index: page as usize - 1,
    });
  }
  Ok(bookmarks)
}

fn replace_bookmarks_in_pdftk_data(data: &str, bookmarks: &[PdfBookmark]) -> String {
  let lines = data.lines().collect::<Vec<_>>();
  let mut output = Vec::<String>::new();
  let mut insert_at = None;
  let mut index = 0;
  while index < lines.len() {
    let line = lines[index];
    if line == "BookmarkBegin" {
      insert_at.get_or_insert(output.len());
      index += 1;
      while index < lines.len() && !lines[index].ends_with("Begin") {
        index += 1;
      }
      continue;
    }
    if insert_at.is_none() && line == "PageMediaBegin" {
      insert_at = Some(output.len());
    }
    output.push(line.to_string());
    index += 1;
  }
  let insert_at = insert_at.unwrap_or(output.len());
  let mut rendered = Vec::with_capacity(output.len() + bookmarks.len() * 4);
  rendered.extend(output[..insert_at].iter().cloned());
  for bookmark in bookmarks {
    rendered.push("BookmarkBegin".to_string());
    rendered.push(format!("BookmarkTitle: {}", bookmark.title));
    rendered.push(format!("BookmarkLevel: {}", bookmark.level.max(1)));
    rendered.push(format!("BookmarkPageNumber: {}", bookmark.page_index + 1));
  }
  rendered.extend(output[insert_at..].iter().cloned());
  let mut body = rendered.join("\n");
  body.push('\n');
  body
}

fn bookmark_signature(bookmarks: &[PdfBookmark]) -> Vec<(u16, usize, &str)> {
  bookmarks
    .iter()
    .map(|bookmark| (bookmark.level, bookmark.page_index, bookmark.title.as_str()))
    .collect()
}

fn temp_unique() -> String {
  let nanos = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_nanos();
  format!("{}-{nanos}", std::process::id())
}

fn temp_pdf_path(path: &Path, unique: String) -> PathBuf {
  let parent = path.parent().unwrap_or_else(|| Path::new("."));
  let stem = path
    .file_stem()
    .map(|stem| stem.to_string_lossy().into_owned())
    .unwrap_or_else(|| "document".to_string());
  parent.join(format!(".{stem}.pdf-tui-bookmarks-{unique}.pdf"))
}

fn toml_string(value: &str) -> String {
  let mut out = String::with_capacity(value.len() + 2);
  out.push('"');
  for ch in value.chars() {
    match ch {
      '\\' => out.push_str("\\\\"),
      '"' => out.push_str("\\\""),
      '\n' => out.push_str("\\n"),
      '\r' => out.push_str("\\r"),
      '\t' => out.push_str("\\t"),
      ch if ch.is_control() => out.push(' '),
      ch => out.push(ch),
    }
  }
  out.push('"');
  out
}
