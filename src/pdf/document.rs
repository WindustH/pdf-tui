use std::{
  fs,
  path::{Path, PathBuf},
  process::Command,
  time::UNIX_EPOCH,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::RenderConfig;

#[derive(Debug, Clone)]
pub struct PdfDocument {
  pub path: PathBuf,
  pub file_name: String,
  pub page_count: usize,
  pub size_bytes: u64,
  pub modified_nanos: u128,
  pub page_cache_dir: PathBuf,
  pub pdftoppm_bin: String,
  pub dpi: u16,
  pub page_size: Option<(u32, u32)>,
}

#[derive(Debug, Clone)]
pub struct PageImage {
  pub page_index: usize,
  pub path: PathBuf,
  pub width: u32,
  pub height: u32,
  pub size_bytes: u64,
  pub modified_nanos: u128,
  pub slice: Option<PageSliceMetadata>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PageSliceId {
  pub page_index: usize,
  pub slice_index: u16,
  pub slice_count: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PageSliceSpec {
  pub page_index: usize,
  pub slice_index: u16,
  pub slice_count: u16,
  pub target_width: u32,
  pub target_height: u32,
  pub slice_y: u32,
  pub slice_height: u32,
  pub cell_width: u16,
  pub cell_height: u16,
  pub full_cell_width: u16,
  pub full_cell_height: u16,
  pub viewport_width: u16,
  pub viewport_height: u16,
  pub scroll_divisor: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSliceMetadata {
  pub source_pdf: String,
  pub source_size_bytes: u64,
  pub source_modified_nanos: String,
  pub page_index: usize,
  pub page_number: usize,
  pub slice_index: u16,
  pub slice_count: u16,
  pub full_pixel_width: u32,
  pub full_pixel_height: u32,
  pub slice_x: u32,
  pub slice_y: u32,
  pub slice_width: u32,
  pub slice_height: u32,
  pub cell_width: u16,
  pub cell_height: u16,
  pub full_cell_width: u16,
  pub full_cell_height: u16,
  pub viewport_width: u16,
  pub viewport_height: u16,
  pub scroll_divisor: u16,
  pub cache_key: String,
}

impl PageSliceSpec {
  pub fn id(self) -> PageSliceId {
    PageSliceId {
      page_index: self.page_index,
      slice_index: self.slice_index,
      slice_count: self.slice_count,
    }
  }

  pub(super) fn normalized(self) -> Self {
    let slice_count = self.slice_count.max(1);
    Self {
      page_index: self.page_index,
      slice_index: self.slice_index.min(slice_count.saturating_sub(1)),
      slice_count,
      target_width: self.target_width.max(1),
      target_height: self.target_height.max(1),
      slice_y: self.slice_y,
      slice_height: self.slice_height.max(1),
      cell_width: self.cell_width.max(1),
      cell_height: self.cell_height.max(1),
      full_cell_width: self.full_cell_width.max(1),
      full_cell_height: self.full_cell_height.max(1),
      viewport_width: self.viewport_width.max(1),
      viewport_height: self.viewport_height.max(1),
      scroll_divisor: self.scroll_divisor.max(1),
    }
  }
}

impl PdfDocument {
  pub fn open(path: PathBuf, cache_dir: PathBuf, render: &RenderConfig) -> Result<Self> {
    let metadata =
      fs::metadata(&path).with_context(|| format!("failed to stat {}", path.display()))?;
    if !metadata.is_file() {
      bail!("{} is not a file", path.display());
    }
    let pdfinfo = read_pdfinfo(&path, &render.pdfinfo_bin)?;
    let page_count = pdfinfo.page_count;
    if page_count == 0 {
      bail!("{} has no pages", path.display());
    }
    let file_name = path
      .file_name()
      .map(|name| name.to_string_lossy().into_owned())
      .unwrap_or_else(|| path.display().to_string());

    Ok(Self {
      path,
      file_name,
      page_count,
      size_bytes: metadata.len(),
      modified_nanos: modified_nanos(&metadata),
      page_cache_dir: cache_dir,
      pdftoppm_bin: render.pdftoppm_bin.clone(),
      dpi: render.page_dpi,
      page_size: pdfinfo.page_size,
    })
  }

  pub fn logical_page_size(&self) -> (u32, u32) {
    self.page_size.unwrap_or((595, 842))
  }

  pub(super) fn cache_key(&self, target_width: u32, target_height: u32) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"pdf-tui-page-v3");
    hasher.update(self.path.to_string_lossy().as_bytes());
    hasher.update(self.size_bytes.to_le_bytes());
    hasher.update(self.modified_nanos.to_le_bytes());
    hasher.update(self.dpi.to_le_bytes());
    hasher.update(target_width.to_le_bytes());
    hasher.update(target_height.to_le_bytes());
    hex::encode(hasher.finalize())
  }

  pub(super) fn slice_cache_key(&self, spec: PageSliceSpec) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"pdf-tui-page-slice-v2");
    hasher.update(self.path.to_string_lossy().as_bytes());
    hasher.update(self.size_bytes.to_le_bytes());
    hasher.update(self.modified_nanos.to_le_bytes());
    hasher.update(self.dpi.to_le_bytes());
    hash_slice_spec(&mut hasher, spec);
    hex::encode(hasher.finalize())
  }
}

struct PdfInfo {
  page_count: usize,
  page_size: Option<(u32, u32)>,
}

fn read_pdfinfo(path: &Path, pdfinfo_bin: &str) -> Result<PdfInfo> {
  let output = Command::new(pdfinfo_bin)
    .arg(path)
    .output()
    .with_context(|| format!("failed to run {pdfinfo_bin}; install poppler-utils"))?;
  if !output.status.success() {
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!("pdfinfo failed: {}", stderr.trim());
  }
  let stdout = String::from_utf8_lossy(&output.stdout);
  let mut page_count = None;
  let mut page_size = None;
  for line in stdout.lines() {
    if let Some(value) = line.strip_prefix("Pages:") {
      page_count = Some(
        value
          .trim()
          .parse::<usize>()
          .context("failed to parse pdfinfo Pages line")?,
      );
    } else if let Some(value) = line.strip_prefix("Page size:") {
      page_size = parse_page_size(value);
    }
  }
  let Some(page_count) = page_count else {
    bail!("pdfinfo did not report a page count");
  };
  Ok(PdfInfo {
    page_count,
    page_size,
  })
}

fn parse_page_size(value: &str) -> Option<(u32, u32)> {
  let (width, rest) = value.trim().split_once('x')?;
  let height = rest.split_whitespace().next()?;
  let width = width.trim().parse::<f64>().ok()?.round().max(1.0) as u32;
  let height = height.trim().parse::<f64>().ok()?.round().max(1.0) as u32;
  Some((width, height))
}

fn hash_slice_spec(hasher: &mut Sha256, spec: PageSliceSpec) {
  let spec = spec.normalized();
  hasher.update(spec.page_index.to_le_bytes());
  hasher.update(spec.slice_index.to_le_bytes());
  hasher.update(spec.slice_count.to_le_bytes());
  hasher.update(spec.target_width.to_le_bytes());
  hasher.update(spec.target_height.to_le_bytes());
  hasher.update(spec.slice_y.to_le_bytes());
  hasher.update(spec.slice_height.to_le_bytes());
  hasher.update(spec.cell_width.to_le_bytes());
  hasher.update(spec.cell_height.to_le_bytes());
  hasher.update(spec.full_cell_width.to_le_bytes());
  hasher.update(spec.full_cell_height.to_le_bytes());
  hasher.update(spec.viewport_width.to_le_bytes());
  hasher.update(spec.viewport_height.to_le_bytes());
  hasher.update(spec.scroll_divisor.to_le_bytes());
}

pub(super) fn modified_nanos(metadata: &fs::Metadata) -> u128 {
  metadata
    .modified()
    .ok()
    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
    .map(|duration| duration.as_nanos())
    .unwrap_or_default()
}
