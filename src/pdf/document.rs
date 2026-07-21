use std::{
  env, fs,
  path::{Path, PathBuf},
  process::Command,
  time::UNIX_EPOCH,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::config::{PdfRasterBackend, RenderConfig};

#[derive(Debug, Clone)]
pub struct PdfDocument {
  pub path: PathBuf,
  pub file_name: String,
  pub page_count: usize,
  pub size_bytes: u64,
  pub modified_nanos: u128,
  pub page_cache_dir: PathBuf,
  pub page_temp_dir: PathBuf,
  pub raster_backend: PdfRasterBackend,
  pub pdf_raster_batch_pages: usize,
  pub pdftoppm_bin: String,
  pub mutool_bin: String,
  pub mutool_band_height: u32,
  pub mutool_threads: usize,
  pub mutool_parallel: bool,
  pub pdfium_library_path: Option<String>,
  pub dpi: u16,
  pub page_sizes: Vec<(u32, u32)>,
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

#[derive(Debug)]
pub struct TempPageImage {
  image: PageImage,
  temp_dir: PathBuf,
}

impl TempPageImage {
  pub(super) fn new(image: PageImage, temp_dir: PathBuf) -> Self {
    Self { image, temp_dir }
  }

  pub fn image(&self) -> &PageImage {
    &self.image
  }
}

impl Drop for TempPageImage {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.temp_dir);
  }
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
      page_temp_dir: env::temp_dir().join("pdf-tui").join("pages"),
      raster_backend: render.pdf_raster_backend,
      pdf_raster_batch_pages: render.pdf_raster_batch_pages.max(1),
      pdftoppm_bin: render.pdftoppm_bin.clone(),
      mutool_bin: render.mutool_bin.clone(),
      mutool_band_height: render.mutool_band_height.max(1),
      mutool_threads: render.mutool_threads.max(1),
      mutool_parallel: render.mutool_parallel,
      pdfium_library_path: render.pdfium_library_path.clone(),
      dpi: render.page_dpi,
      page_sizes: pdfinfo.page_sizes,
    })
  }

  pub fn logical_page_size(&self, index: usize) -> (u32, u32) {
    self
      .page_sizes
      .get(index)
      .copied()
      .unwrap_or(default_page_size())
  }

  pub(super) fn cache_key(&self, target_width: u32, target_height: u32) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"pdf-tui-page-v6");
    hasher.update(self.raster_backend.label().as_bytes());
    hasher.update([0]);
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
    hasher.update(b"pdf-tui-page-slice-v5");
    hasher.update(self.raster_backend.label().as_bytes());
    hasher.update([0]);
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
  page_sizes: Vec<(u32, u32)>,
}

fn read_pdfinfo(path: &Path, pdfinfo_bin: &str) -> Result<PdfInfo> {
  let stdout = run_pdfinfo(path, pdfinfo_bin, &[])?;
  let mut page_count = None;
  let mut default_size = None;
  for line in stdout.lines() {
    if let Some(value) = line.strip_prefix("Pages:") {
      page_count = Some(
        value
          .trim()
          .parse::<usize>()
          .context("failed to parse pdfinfo Pages line")?,
      );
    } else if let Some(value) = line.strip_prefix("Page size:") {
      default_size = parse_page_size(value);
    }
  }
  let Some(page_count) = page_count else {
    bail!("pdfinfo did not report a page count");
  };
  let fallback = default_size.unwrap_or_else(default_page_size);
  let page_sizes = match read_pdfinfo_page_sizes(path, pdfinfo_bin, page_count, fallback) {
    Ok(page_sizes) => page_sizes,
    Err(error) => {
      warn!(
        path = %path.display(),
        %error,
        "failed to read per-page PDF sizes; falling back to default page size"
      );
      vec![fallback; page_count]
    }
  };
  Ok(PdfInfo {
    page_count,
    page_sizes,
  })
}

fn run_pdfinfo(path: &Path, pdfinfo_bin: &str, args: &[String]) -> Result<String> {
  let output = Command::new(pdfinfo_bin)
    .args(args)
    .arg(path)
    .output()
    .with_context(|| format!("failed to run {pdfinfo_bin}; install poppler-utils"))?;
  if !output.status.success() {
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!("pdfinfo failed: {}", stderr.trim());
  }
  Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn read_pdfinfo_page_sizes(
  path: &Path,
  pdfinfo_bin: &str,
  page_count: usize,
  fallback: (u32, u32),
) -> Result<Vec<(u32, u32)>> {
  if page_count == 0 {
    return Ok(Vec::new());
  }
  let stdout = run_pdfinfo(
    path,
    pdfinfo_bin,
    &[
      "-f".to_string(),
      "1".to_string(),
      "-l".to_string(),
      page_count.to_string(),
    ],
  )?;
  let mut page_sizes = vec![fallback; page_count];
  for line in stdout.lines() {
    if let Some((index, size)) = parse_numbered_page_size(line)
      && let Some(slot) = page_sizes.get_mut(index)
    {
      *slot = size;
    }
  }
  Ok(page_sizes)
}

fn parse_numbered_page_size(line: &str) -> Option<(usize, (u32, u32))> {
  let line = line.strip_prefix("Page")?.trim_start();
  let (number, rest) = line.split_once("size:")?;
  let page_number = number.trim().parse::<usize>().ok()?;
  let size = parse_page_size(rest)?;
  page_number.checked_sub(1).map(|index| (index, size))
}

fn parse_page_size(value: &str) -> Option<(u32, u32)> {
  let (width, rest) = value.trim().split_once('x')?;
  let height = rest.split_whitespace().next()?;
  let width = width.trim().parse::<f64>().ok()?.round().max(1.0) as u32;
  let height = height.trim().parse::<f64>().ok()?.round().max(1.0) as u32;
  Some((width, height))
}

fn default_page_size() -> (u32, u32) {
  (595, 842)
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
