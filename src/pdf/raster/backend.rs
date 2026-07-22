use std::{
  collections::HashMap,
  env,
  path::{Path, PathBuf},
  sync::{Mutex as StdMutex, OnceLock},
};

use anyhow::{Context, Result, bail};
use image::ImageFormat;
use pdfium_render::prelude::*;
use tokio::process::Command;

use crate::config::PdfRasterBackend;

use super::super::document::PdfDocument;
use super::{
  crop::SelectionCropPlan,
  file_cache::{TempWorkDir, collect_numbered_png_outputs, single_png_path_for_prefix},
};

static PDFIUM_RENDER_LOCK: StdMutex<()> = StdMutex::new(());
static PDFIUM_INITIALIZED: OnceLock<()> = OnceLock::new();
const PDF_TUI_PDFIUM_LIBRARY_PATH_ENV: &str = "PDF_TUI_PDFIUM_LIBRARY_PATH";

#[allow(clippy::too_many_arguments)]
pub(super) async fn render_pdf_page_batch(
  document: &PdfDocument,
  missing: &[usize],
  first_page: usize,
  last_page: usize,
  target_width: u32,
  target_height: u32,
  temp_dir: &TempWorkDir,
  temp_prefix: &Path,
) -> Result<HashMap<usize, PathBuf>> {
  match document.raster_backend {
    PdfRasterBackend::Poppler => {
      render_poppler_page_batch(
        document,
        first_page,
        last_page,
        target_width,
        target_height,
        temp_dir,
        temp_prefix,
      )
      .await
    }
    PdfRasterBackend::Mutool => {
      render_mutool_page_batch(
        document,
        first_page,
        last_page,
        target_width,
        target_height,
        temp_prefix,
      )
      .await
    }
    PdfRasterBackend::Pdfium => {
      render_pdfium_pages(
        document,
        missing,
        target_width,
        target_height,
        temp_dir.path(),
      )
      .await
    }
  }
}

#[allow(clippy::too_many_arguments)]
async fn render_poppler_page_batch(
  document: &PdfDocument,
  first_page: usize,
  last_page: usize,
  target_width: u32,
  target_height: u32,
  temp_dir: &TempWorkDir,
  temp_prefix: &Path,
) -> Result<HashMap<usize, PathBuf>> {
  if first_page == last_page {
    let temp_prefix = temp_dir.path().join(format!("page-{first_page:05}"));
    let temp_output = single_png_path_for_prefix(&temp_prefix);
    run_pdftoppm_single(
      document,
      first_page,
      target_width,
      target_height,
      &temp_prefix,
    )
    .await?;
    Ok(HashMap::from([(first_page, temp_output)]))
  } else {
    run_pdftoppm_batch(
      document,
      first_page,
      last_page,
      target_width,
      target_height,
      temp_prefix,
    )
    .await?;
    collect_numbered_png_outputs(temp_prefix, first_page, last_page).await
  }
}

async fn run_pdftoppm_batch(
  document: &PdfDocument,
  first_page: usize,
  last_page: usize,
  target_width: u32,
  target_height: u32,
  temp_prefix: &Path,
) -> Result<()> {
  let output = Command::new(&document.pdftoppm_bin)
    .arg("-f")
    .arg(first_page.to_string())
    .arg("-l")
    .arg(last_page.to_string())
    .arg("-scale-to-x")
    .arg(target_width.to_string())
    .arg("-scale-to-y")
    .arg(target_height.to_string())
    .arg("-png")
    .arg(&document.path)
    .arg(temp_prefix)
    .output()
    .await
    .with_context(|| {
      format!(
        "failed to run {}; install poppler-utils",
        document.pdftoppm_bin
      )
    })?;
  if !output.status.success() {
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!(
      "pdftoppm failed for pages {first_page}-{last_page}: {}",
      stderr.trim()
    );
  }
  Ok(())
}

async fn run_pdftoppm_single(
  document: &PdfDocument,
  page_number: usize,
  target_width: u32,
  target_height: u32,
  temp_prefix: &Path,
) -> Result<()> {
  let output = Command::new(&document.pdftoppm_bin)
    .arg("-f")
    .arg(page_number.to_string())
    .arg("-l")
    .arg(page_number.to_string())
    .arg("-singlefile")
    .arg("-scale-to-x")
    .arg(target_width.to_string())
    .arg("-scale-to-y")
    .arg(target_height.to_string())
    .arg("-png")
    .arg(&document.path)
    .arg(temp_prefix)
    .output()
    .await
    .with_context(|| {
      format!(
        "failed to run {}; install poppler-utils",
        document.pdftoppm_bin
      )
    })?;
  if !output.status.success() {
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!("pdftoppm failed for page {page_number}: {}", stderr.trim());
  }
  Ok(())
}

pub(super) async fn render_poppler_selection_image(
  document: &PdfDocument,
  page_number: usize,
  plan: SelectionCropPlan,
  temp_dir: &TempWorkDir,
) -> Result<PathBuf> {
  let temp_prefix = temp_dir.path().join(format!(
    "selection-{page_number:05}-{}x{}",
    plan.crop.width, plan.crop.height
  ));
  let output_path = single_png_path_for_prefix(&temp_prefix);
  let output = Command::new(&document.pdftoppm_bin)
    .arg("-f")
    .arg(page_number.to_string())
    .arg("-l")
    .arg(page_number.to_string())
    .arg("-singlefile")
    .arg("-scale-to-x")
    .arg(plan.page_width.to_string())
    .arg("-scale-to-y")
    .arg(plan.page_height.to_string())
    .arg("-x")
    .arg(plan.crop.x.to_string())
    .arg("-y")
    .arg(plan.crop.y.to_string())
    .arg("-W")
    .arg(plan.crop.width.to_string())
    .arg("-H")
    .arg(plan.crop.height.to_string())
    .arg("-png")
    .arg(&document.path)
    .arg(&temp_prefix)
    .output()
    .await
    .with_context(|| {
      format!(
        "failed to run {}; install poppler-utils",
        document.pdftoppm_bin
      )
    })?;
  if !output.status.success() {
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!(
      "pdftoppm failed for page {page_number} crop {}x{}+{}+{}: {}",
      plan.crop.width,
      plan.crop.height,
      plan.crop.x,
      plan.crop.y,
      stderr.trim()
    );
  }
  Ok(output_path)
}

async fn render_mutool_page_batch(
  document: &PdfDocument,
  first_page: usize,
  last_page: usize,
  target_width: u32,
  target_height: u32,
  temp_prefix: &Path,
) -> Result<HashMap<usize, PathBuf>> {
  let output_pattern = temp_prefix.with_file_name(format!(
    "{}-%d.png",
    temp_prefix
      .file_name()
      .and_then(|name| name.to_str())
      .unwrap_or("page")
  ));
  let mut command = Command::new(&document.mutool_bin);
  command
    .arg("draw")
    .arg("-q")
    .arg("-F")
    .arg("png")
    .arg("-o")
    .arg(&output_pattern)
    .arg("-w")
    .arg(target_width.max(1).to_string())
    .arg("-h")
    .arg(target_height.max(1).to_string())
    .arg("-f")
    .arg("-B")
    .arg(document.mutool_band_height.max(1).to_string())
    .arg("-T")
    .arg(document.mutool_threads.max(1).to_string());
  if document.mutool_parallel {
    command.arg("-P");
  }
  let pages = if first_page == last_page {
    first_page.to_string()
  } else {
    format!("{first_page}-{last_page}")
  };
  let output = command
    .arg(&document.path)
    .arg(&pages)
    .output()
    .await
    .with_context(|| format!("failed to run {}; install mupdf-tools", document.mutool_bin))?;
  if !output.status.success() {
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!(
      "mutool failed for pages {first_page}-{last_page}: {}",
      stderr.trim()
    );
  }
  collect_numbered_png_outputs(temp_prefix, first_page, last_page).await
}

pub(super) async fn render_pdfium_selection_image(
  document: &PdfDocument,
  page_number: usize,
  plan: SelectionCropPlan,
  temp_dir: &Path,
) -> Result<PathBuf> {
  let pdf_path = document.path.clone();
  let library_path = document.pdfium_library_path.clone();
  let output_path = temp_dir.join(format!(
    "selection-{page_number:05}-{}x{}+{}+{}.png",
    plan.crop.width, plan.crop.height, plan.crop.x, plan.crop.y
  ));
  tokio::task::spawn_blocking(move || {
    render_pdfium_selection_image_blocking(pdf_path, library_path, page_number, plan, output_path)
  })
  .await
  .map_err(|error| anyhow::anyhow!("pdfium selection worker failed: {error}"))?
}

fn render_pdfium_selection_image_blocking(
  pdf_path: PathBuf,
  library_path: Option<String>,
  page_number: usize,
  plan: SelectionCropPlan,
  output_path: PathBuf,
) -> Result<PathBuf> {
  let _guard = PDFIUM_RENDER_LOCK
    .lock()
    .map_err(|error| anyhow::anyhow!("pdfium render lock poisoned: {error}"))?;
  let pdfium = pdfium_instance(library_path.as_deref())?;
  let document = pdfium
    .load_pdf_from_file(&pdf_path, None)
    .with_context(|| format!("failed to open {}", pdf_path.display()))?;
  let page_index =
    i32::try_from(page_number.saturating_sub(1)).context("pdfium page index is too large")?;
  let page = document
    .pages()
    .get(page_index)
    .with_context(|| format!("failed to load pdfium page {page_number}"))?;
  let full_width =
    i32::try_from(plan.page_width.max(1)).context("pdfium target width is too large")?;
  let full_height =
    i32::try_from(plan.page_height.max(1)).context("pdfium target height is too large")?;
  let crop_width =
    i32::try_from(plan.crop.width.max(1)).context("pdfium crop width is too large")?;
  let crop_height =
    i32::try_from(plan.crop.height.max(1)).context("pdfium crop height is too large")?;
  let crop_x = i32::try_from(plan.crop.x).context("pdfium crop x is too large")?;
  let crop_y = i32::try_from(plan.crop.y).context("pdfium crop y is too large")?;
  let mut bitmap = PdfBitmap::empty(crop_width, crop_height, PdfBitmapFormat::BGRA)
    .context("failed to allocate pdfium selection bitmap")?;
  let render_config = PdfRenderConfig::new()
    .set_fixed_size(full_width, full_height)
    .set_origin(-crop_x, -crop_y);
  page
    .render_into_bitmap_with_config(&mut bitmap, &render_config)
    .with_context(|| format!("failed to render pdfium selection on page {page_number}"))?;
  bitmap
    .as_image()
    .with_context(|| format!("failed to convert pdfium selection on page {page_number} to image"))?
    .save_with_format(&output_path, ImageFormat::Png)
    .with_context(|| format!("failed to write {}", output_path.display()))?;
  Ok(output_path)
}

async fn render_pdfium_pages(
  document: &PdfDocument,
  missing: &[usize],
  target_width: u32,
  target_height: u32,
  temp_dir: &Path,
) -> Result<HashMap<usize, PathBuf>> {
  let page_numbers = missing
    .iter()
    .map(|index| index.saturating_add(1))
    .collect::<Vec<_>>();
  let pdf_path = document.path.clone();
  let library_path = document.pdfium_library_path.clone();
  let temp_dir = temp_dir.to_path_buf();
  tokio::task::spawn_blocking(move || {
    render_pdfium_pages_blocking(
      pdf_path,
      library_path,
      page_numbers,
      target_width,
      target_height,
      temp_dir,
    )
  })
  .await
  .map_err(|error| anyhow::anyhow!("pdfium worker failed: {error}"))?
}

fn render_pdfium_pages_blocking(
  pdf_path: PathBuf,
  library_path: Option<String>,
  page_numbers: Vec<usize>,
  target_width: u32,
  target_height: u32,
  temp_dir: PathBuf,
) -> Result<HashMap<usize, PathBuf>> {
  let _guard = PDFIUM_RENDER_LOCK
    .lock()
    .map_err(|error| anyhow::anyhow!("pdfium render lock poisoned: {error}"))?;
  let pdfium = pdfium_instance(library_path.as_deref())?;
  let document = pdfium
    .load_pdf_from_file(&pdf_path, None)
    .with_context(|| format!("failed to open {}", pdf_path.display()))?;
  let width = i32::try_from(target_width.max(1)).context("pdfium target width is too large")?;
  let height = i32::try_from(target_height.max(1)).context("pdfium target height is too large")?;
  let render_config = PdfRenderConfig::new().set_fixed_size(width, height);
  let mut outputs = HashMap::new();
  for page_number in page_numbers {
    let page_index =
      i32::try_from(page_number.saturating_sub(1)).context("pdfium page index is too large")?;
    let page = document
      .pages()
      .get(page_index)
      .with_context(|| format!("failed to load pdfium page {page_number}"))?;
    let image = page
      .render_with_config(&render_config)
      .with_context(|| format!("failed to render pdfium page {page_number}"))?
      .as_image()
      .with_context(|| format!("failed to convert pdfium page {page_number} to image"))?;
    let output_path = temp_dir.join(format!("page-{page_number}.png"));
    image
      .save_with_format(&output_path, ImageFormat::Png)
      .with_context(|| format!("failed to write {}", output_path.display()))?;
    outputs.insert(page_number, output_path);
  }
  Ok(outputs)
}

fn pdfium_instance(library_path: Option<&str>) -> Result<Pdfium> {
  if PDFIUM_INITIALIZED.get().is_some() {
    return Ok(Pdfium::default());
  }

  let mut errors = Vec::new();
  for library in pdfium_library_candidates(library_path) {
    match Pdfium::bind_to_library(&library) {
      Ok(bindings) => {
        let pdfium = Pdfium::new(bindings);
        let _ = PDFIUM_INITIALIZED.set(());
        return Ok(pdfium);
      }
      Err(PdfiumError::PdfiumLibraryBindingsAlreadyInitialized) => {
        let _ = PDFIUM_INITIALIZED.set(());
        return Ok(Pdfium::default());
      }
      Err(error) => errors.push(format!("{}: {error}", library.display())),
    }
  }

  match Pdfium::bind_to_system_library() {
    Ok(bindings) => {
      let pdfium = Pdfium::new(bindings);
      let _ = PDFIUM_INITIALIZED.set(());
      Ok(pdfium)
    }
    Err(PdfiumError::PdfiumLibraryBindingsAlreadyInitialized) => {
      let _ = PDFIUM_INITIALIZED.set(());
      Ok(Pdfium::default())
    }
    Err(error) => {
      if errors.is_empty() {
        Err(error).context("failed to bind to pdfium")
      } else {
        Err(anyhow::anyhow!(
          "failed to bind to pdfium; tried {}; system library: {error}",
          errors.join("; ")
        ))
      }
    }
  }
}

fn pdfium_library_candidates(configured: Option<&str>) -> Vec<PathBuf> {
  let mut candidates = Vec::new();
  if let Some(path) = configured.filter(|path| !path.trim().is_empty()) {
    push_pdfium_candidate(&mut candidates, PathBuf::from(path));
  }
  if let Some(path) = env::var_os(PDF_TUI_PDFIUM_LIBRARY_PATH_ENV).filter(|path| !path.is_empty()) {
    push_pdfium_candidate(&mut candidates, PathBuf::from(path));
  }
  for path in packaged_pdfium_candidates() {
    if path.exists() {
      push_pdfium_candidate(&mut candidates, path);
    }
  }
  candidates
}

fn push_pdfium_candidate(candidates: &mut Vec<PathBuf>, path: PathBuf) {
  let library = if path.is_dir() {
    Pdfium::pdfium_platform_library_name_at_path(&path)
  } else {
    path
  };
  if !candidates.iter().any(|candidate| candidate == &library) {
    candidates.push(library);
  }
}

fn packaged_pdfium_candidates() -> Vec<PathBuf> {
  let mut out = Vec::new();
  let Some(exe) = env::current_exe().ok() else {
    return out;
  };
  let Some(dir) = exe.parent() else {
    return out;
  };
  let library_name = PathBuf::from(Pdfium::pdfium_platform_library_name());
  out.push(dir.join("pdfium").join("lib").join(&library_name));
  if let Some(parent) = dir.parent() {
    out.push(parent.join("pdfium").join("lib").join(&library_name));
  }
  out
}
