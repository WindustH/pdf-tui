mod app;
mod background;
mod bookmarks;
mod cache;
mod clipboard;
mod config;
mod event;
mod event_loop;
mod geometry;
mod job_queue;
mod layout;
mod logging;
mod metadata;
mod overlay;
mod pdf;
mod progress_store;
mod render;
mod search;
mod selection;
mod terminal;
mod ui;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use img_tui::{NativeImageConfig, RenderMode, TerminalCapability, capability, native_image};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::{
  app::App,
  background::InputGate,
  config::{RenderConfig, Settings},
  event::AsyncEvent,
  event_loop::Session,
  overlay::OverlayStore,
  pdf::{PageStore, PdfDocument},
  render::RenderStore,
  terminal::Tui,
  ui::ImagePipeline,
};

#[derive(Debug, Parser)]
#[command(version, about = "Read PDFs in a terminal UI")]
struct Cli {
  /// Initial 0-based reading progress, e.g. 0.0 is the top of the first page.
  #[arg(long)]
  progress: Option<f64>,

  /// PDF file to open.
  path: PathBuf,

  /// Optional layout override: scroll <columns> <scroll_divisor> or grid <rows> <columns>.
  #[arg(trailing_var_arg = true)]
  layout: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
  let cli = Cli::parse();
  let input = cli
    .path
    .canonicalize()
    .with_context(|| format!("failed to resolve {}", cli.path.display()))?;

  let mut settings = config::load_or_create().await?;
  let _cache_instance = cache::register_instance(&settings.cache_dir)?;
  let log_path = logging::init(&settings.cache_dir)?;
  eprintln!("pdf-tui log: {}", log_path.display());
  info!(
    input = %input.display(),
    cache_dir = %settings.cache_dir.display(),
    config_path = %settings.config_path.display(),
    log_path = %log_path.display(),
    "pdf-tui starting"
  );
  tidy_cache(&settings).await;
  apply_cli_layout(&mut settings, &cli.layout)?;

  let terminal_capability = capability::detect();
  info!(?terminal_capability, "detected terminal capability");
  let (effective_render, render_modes) =
    render_setup(&settings.config.render, &terminal_capability);

  let document = PdfDocument::open(
    input,
    settings.cache_dir.join("pages"),
    &settings.config.render,
  )?;
  info!(
    path = %document.path.display(),
    file_name = %document.file_name,
    page_count = document.page_count,
    dpi = document.dpi,
    "opened pdf document"
  );
  let remember_position = settings.config.behavior.remember_reading_position;
  let saved_progress = if remember_position && document.modified_nanos > 0 {
    progress_store::load_matching(
      &settings.cache_dir,
      &document.path,
      document.size_bytes,
      document.modified_nanos,
    )
  } else {
    None
  };
  let page_store = PageStore::new(document.clone(), settings.config.render.max_concurrent);

  let (tx, rx) = mpsc::unbounded_channel::<AsyncEvent>();
  let input_gate = InputGate::spawn(tx.clone());
  background::spawn_file_watcher(tx.clone(), document.path.clone(), &settings.config.behavior);

  let mut app = App::new(document, settings);
  app.terminal_cell_pixels = terminal_capability.cell_pixels;
  if let Some(progress) = cli.progress.or(saved_progress) {
    app.set_progress_target(progress);
  }

  let native_config = NativeImageConfig {
    cell_pixels: terminal_capability.cell_pixels,
    passthrough: terminal_capability.passthrough().map(str::to_string),
    kitty_unicode_placeholders: terminal_capability.kitty_unicode_placeholders(),
  };
  let protocol_reset = render_modes
    .contains(&RenderMode::Kitty)
    .then(|| {
      native_image::erase_sequence(
        RenderMode::Kitty,
        native_config.passthrough.as_deref(),
        None,
      )
    })
    .flatten();
  let pipeline = ImagePipeline {
    pages: page_store,
    overlays: OverlayStore::new(app.settings.cache_dir.clone(), &app.settings.config.render),
    renderer: RenderStore::new(
      app.settings.cache_dir.join("render"),
      effective_render,
      native_config,
      render_modes,
    ),
  };

  terminal::install_panic_hook();
  let mut tui = Tui::new(protocol_reset)?;
  let mut session = Session::new(app, pipeline, tx, rx, input_gate);
  let result = session.run(&mut tui).await;
  tui.restore()?;
  result?;
  if remember_position {
    save_reading_position(&session.app);
  }
  Ok(())
}

/// Removes the obsolete crop cache and trims the disk cache to its limit.
/// Failures are reported but never block startup.
async fn tidy_cache(settings: &Settings) {
  if let Err(error) = cache::remove_legacy_crop_cache(&settings.cache_dir).await {
    warn!(%error, "failed to remove legacy crop cache");
    eprintln!("failed to remove legacy crop cache: {error}");
  }
  if let Err(error) =
    cache::enforce_render_cache_limit(&settings.cache_dir, settings.config.render.cache_max_bytes)
      .await
  {
    warn!(%error, "failed to clean pdf-tui cache");
    eprintln!("failed to clean pdf-tui cache: {error}");
  }
}

/// Chafa arguments adjusted to the detected terminal, and the order in
/// which render modes are tried.
fn render_setup(
  configured: &RenderConfig,
  terminal_capability: &TerminalCapability,
) -> (RenderConfig, Vec<RenderMode>) {
  let mut effective_render = configured.clone();
  if effective_render.auto_detect {
    effective_render.apply_terminal_capability(terminal_capability);
  }
  let render_modes = if let Some(modes) = capability::render_modes_override_from_env() {
    modes
  } else if effective_render.auto_detect {
    terminal_capability.preferred_render_modes(&effective_render.zellij_sixel)
  } else {
    vec![RenderMode::Symbols, RenderMode::Ascii]
  };
  info!(
    modes = ?render_modes.iter().map(|mode| mode.label()).collect::<Vec<_>>(),
    effective_render = ?effective_render,
    "render mode order"
  );
  (effective_render, render_modes)
}

/// Saves the reading position keyed by the document as it is now: after a
/// refresh or an in-app edit, the file on disk is the reloaded version.
fn save_reading_position(app: &App) {
  let document = &app.document;
  if document.modified_nanos == 0 {
    return;
  }
  let Some(progress) = app.save_progress_on_exit() else {
    return;
  };
  let entry = progress_store::ProgressEntry::new(
    &document.path,
    progress,
    document.size_bytes,
    document.modified_nanos,
    document.page_count,
  );
  if let Err(error) = progress_store::upsert(&app.settings.cache_dir, entry) {
    warn!(%error, "could not persist reading progress");
  }
}

fn apply_cli_layout(settings: &mut Settings, args: &[String]) -> Result<()> {
  let Some((name, raw_args)) = args.split_first() else {
    return Ok(());
  };
  let raw_args = raw_args.iter().map(String::as_str).collect::<Vec<_>>();
  settings
    .config
    .layout
    .set_active_from_args(name, &raw_args)
    .map_err(anyhow::Error::msg)?;
  Ok(())
}
