mod app;
mod bookmarks;
mod cache;
mod clipboard;
mod config;
mod event;
mod layout;
mod logging;
mod metadata;
mod pdf;
mod render;
mod search;
mod selection;
mod terminal;
mod ui;

use std::{
  fs,
  path::PathBuf,
  sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
  },
  thread,
  time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event as crossterm_event;
use framework_tui::edit_text_in_editor;
use img_tui::{NativeImageConfig, RenderMode, capability, native_image};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::{
  app::{App, EditorRequest},
  event::AsyncEvent,
  pdf::{PageStore, PdfDocument},
  render::RenderStore,
  terminal::Tui,
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
  apply_cli_layout(&mut settings, &cli.layout)?;

  let terminal_capability = capability::detect();
  info!(?terminal_capability, "detected terminal capability");
  let mut effective_render = settings.config.render.clone();
  if effective_render.auto_detect {
    effective_render.apply_terminal_capability(&terminal_capability);
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
  let mut page_store = PageStore::new(document.clone(), settings.config.render.max_concurrent);

  let (tx, mut rx) = mpsc::unbounded_channel::<AsyncEvent>();
  let input_enabled = Arc::new(AtomicBool::new(true));
  let input_generation = Arc::new(AtomicU64::new(0));
  spawn_input_thread(tx.clone(), input_enabled.clone(), input_generation.clone());
  spawn_auto_refresh_thread(
    tx.clone(),
    document.path.clone(),
    settings.config.behavior.clone(),
  );

  let mut app = App::new(document, settings);
  app.terminal_cell_pixels = terminal_capability.cell_pixels;
  if let Some(progress) = cli.progress {
    app.set_user_progress_target(progress);
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
  let mut renderer = RenderStore::new(
    app.settings.cache_dir.join("render"),
    effective_render,
    native_config,
    render_modes,
  );

  let mut tui = Tui::new(protocol_reset)?;
  let mut needs_draw = true;
  loop {
    if needs_draw {
      debug!(scroll = app.scroll, focused_page = app.focused_page, layout = %app.layout.label(), "drawing frame");
      tui.draw(|frame| ui::draw(frame, &mut app, &mut page_store, &mut renderer, &tx))?;
      needs_draw = false;
      if app.should_quit() {
        break;
      }
    }

    if let Some(request) = app.take_editor_request() {
      input_enabled.store(false, Ordering::SeqCst);
      input_generation.fetch_add(1, Ordering::SeqCst);
      tui.suspend()?;
      let result = edit_text_in_editor(request.initial_text(), &app.settings.cache_dir);
      let resume_result = tui.resume();
      if resume_result.is_ok() {
        discard_pending_terminal_events();
      }
      input_generation.fetch_add(1, Ordering::SeqCst);
      input_enabled.store(true, Ordering::SeqCst);
      match request {
        EditorRequest::Metadata { original, .. } => {
          app.finish_metadata_editor_input(original, result)
        }
        EditorRequest::Bookmarks { original, .. } => {
          app.finish_bookmarks_editor_input(original, result)
        }
      }
      resume_result?;
      needs_draw = true;
      continue;
    }

    let Some(message) = rx.recv().await else {
      break;
    };
    needs_draw |= handle_async_event(
      message,
      &input_generation,
      &mut app,
      &mut page_store,
      &mut renderer,
      &tx,
    );
    while let Ok(message) = rx.try_recv() {
      needs_draw |= handle_async_event(
        message,
        &input_generation,
        &mut app,
        &mut page_store,
        &mut renderer,
        &tx,
      );
    }
  }
  tui.restore()?;
  Ok(())
}

fn handle_async_event(
  message: AsyncEvent,
  input_generation: &AtomicU64,
  app: &mut App,
  page_store: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
) -> bool {
  match message {
    AsyncEvent::Input { event, generation } => {
      let current_generation = input_generation.load(Ordering::SeqCst);
      if generation == current_generation {
        debug!(?event, generation, "input event accepted");
        let redraw = app.handle_input(event, tx);
        if app.take_search_preload_reset() {
          page_store.cancel_preloads();
          renderer.cancel_preloads();
        }
        debug!(
          redraw,
          scroll = app.scroll,
          focused_page = app.focused_page,
          message = %app.message,
          "input event handled"
        );
        redraw
      } else {
        debug!(
          ?event,
          generation, current_generation, "input event ignored because generation is stale"
        );
        false
      }
    }
    AsyncEvent::Page(outcome) => {
      if outcome.source_size_bytes != app.document.size_bytes
        || outcome.source_modified_nanos != app.document.modified_nanos
      {
        debug!(
          page = outcome.page_index + 1,
          source_size_bytes = outcome.source_size_bytes,
          current_size_bytes = app.document.size_bytes,
          source_modified_nanos = outcome.source_modified_nanos,
          current_modified_nanos = app.document.modified_nanos,
          "ignored stale page outcome"
        );
        return false;
      }
      let page_index = outcome.page_index;
      let preload = outcome.preload;
      let success = outcome.result.is_ok();
      let slice = outcome.slice;
      match &outcome.result {
        Ok(page) => {
          if let Some(slice) = slice {
            debug!(
              page = page_index + 1,
              slice = slice.slice_index + 1,
              slice_count = slice.slice_count,
              preload,
              width = page.width,
              height = page.height,
              path = %page.path.display(),
              metadata = ?page.slice,
              "page slice render completed"
            );
          } else {
            debug!(
              page = page_index + 1,
              preload,
              width = page.width,
              height = page.height,
              path = %page.path.display(),
              "page render completed"
            );
          }
        }
        Err(error) => {
          if let Some(slice) = slice {
            warn!(
              page = page_index + 1,
              slice = slice.slice_index + 1,
              slice_count = slice.slice_count,
              preload,
              %error,
              "page slice render failed"
            );
          } else {
            warn!(page = page_index + 1, preload, %error, "page render failed");
          }
        }
      }
      let visible_wait = page_store.finish(&outcome, success || !preload, tx);
      if success || !preload {
        if let Some(slice) = slice {
          app.finish_slice(slice, outcome.result);
        } else {
          app.finish_page(page_index, outcome.result);
        }
        ui::pump_preload(app, page_store, renderer, tx);
        let redraw = !preload || visible_wait;
        debug!(
          page = page_index + 1,
          preload, visible_wait, redraw, "page event handled"
        );
        redraw
      } else {
        debug!(
          page = page_index + 1,
          preload, visible_wait, "page preload event handled"
        );
        visible_wait
      }
    }
    AsyncEvent::Render(outcome) => {
      match &outcome.result {
        Ok(rendered) => debug!(
          cache_key = %outcome.cache_key,
          slot_key = %outcome.slot_key,
          preload = outcome.preload,
          kind = rendered_kind(rendered),
          "image render completed"
        ),
        Err(error) => warn!(
          cache_key = %outcome.cache_key,
          slot_key = %outcome.slot_key,
          preload = outcome.preload,
          %error,
          "image render failed"
        ),
      }
      let result = renderer.finish(outcome, tx);
      if let Some(error) = result.message {
        app.set_message(error);
      }
      debug!(needs_draw = result.needs_draw, "image render event handled");
      result.needs_draw
    }
    AsyncEvent::AutoRefreshRequested => {
      app.request_refresh(tx);
      true
    }
    AsyncEvent::Refresh(outcome) => {
      let queued = app.finish_refresh_request();
      match outcome.result {
        Ok(reload) => {
          apply_document_reload(reload, app, page_store, renderer);
          app.set_message("refreshed current document");
          info!(path = %app.document.path.display(), "document refreshed");
        }
        Err(error) => {
          app.set_message(format!("refresh failed: {error}"));
          warn!(%error, "document refresh failed");
        }
      }
      if queued {
        app.request_refresh(tx);
      }
      true
    }
    AsyncEvent::MetadataWrite(outcome) => match outcome.result {
      Ok(reload) => {
        apply_document_reload(reload, app, page_store, renderer);
        app.set_message(format!("metadata updated: {} tag(s)", outcome.changed_tags));
        info!(
          changed_tags = outcome.changed_tags,
          "metadata write finished"
        );
        true
      }
      Err(error) => {
        app.set_message(format!("metadata write failed: {error}"));
        warn!(%error, "metadata write failed");
        true
      }
    },
    AsyncEvent::BookmarksWrite(outcome) => match outcome.result {
      Ok(reload) => {
        apply_document_reload(reload, app, page_store, renderer);
        app.set_message(format!(
          "bookmarks updated: {} entries",
          outcome.changed_bookmarks
        ));
        info!(
          changed_bookmarks = outcome.changed_bookmarks,
          "bookmarks write finished"
        );
        true
      }
      Err(error) => {
        app.set_message(format!("bookmarks write failed: {error}"));
        warn!(%error, "bookmarks write failed");
        true
      }
    },
    AsyncEvent::SearchIndex(outcome) => {
      if outcome.source_size_bytes != app.document.size_bytes
        || outcome.source_modified_nanos != app.document.modified_nanos
      {
        debug!(
          source_size_bytes = outcome.source_size_bytes,
          current_size_bytes = app.document.size_bytes,
          source_modified_nanos = outcome.source_modified_nanos,
          current_modified_nanos = app.document.modified_nanos,
          "ignored stale search index"
        );
        return false;
      }
      app.finish_search_index(outcome.result);
      app.finish_pending_selection_text_copy(tx);
      if app.take_search_preload_reset() {
        page_store.cancel_preloads();
        renderer.cancel_preloads();
      }
      ui::pump_preload(app, page_store, renderer, tx);
      true
    }
    AsyncEvent::SearchPreloadReady { generation } => {
      if app.finish_search_preload_delay(generation) {
        ui::pump_preload(app, page_store, renderer, tx);
      }
      false
    }
    AsyncEvent::SelectionImage(outcome) => {
      if outcome.source_size_bytes != app.document.size_bytes
        || outcome.source_modified_nanos != app.document.modified_nanos
      {
        debug!(
          source_size_bytes = outcome.source_size_bytes,
          current_size_bytes = app.document.size_bytes,
          source_modified_nanos = outcome.source_modified_nanos,
          current_modified_nanos = app.document.modified_nanos,
          "ignored stale selection image"
        );
        return false;
      }
      let redraw = !outcome.preload || app.view == app::ViewMode::Selection;
      app.finish_selection_image(outcome);
      ui::pump_preload(app, page_store, renderer, tx);
      redraw
    }
    AsyncEvent::Clipboard(outcome) => {
      app.finish_clipboard(outcome);
      true
    }
    AsyncEvent::CacheClear(outcome) => match outcome.result {
      Ok(report) => {
        app.clear_cached_images();
        page_store.clear_state();
        renderer.clear_state();
        app.set_message(format!(
          "cache cleared: {} files, {} bytes",
          report.removed_files, report.removed_bytes
        ));
        info!(
          before_bytes = report.before_bytes,
          after_bytes = report.after_bytes,
          removed_files = report.removed_files,
          removed_bytes = report.removed_bytes,
          "cache cleared"
        );
        true
      }
      Err(error) => {
        app.set_message(format!("clear-cache failed: {error}"));
        warn!(%error, "cache clear failed");
        true
      }
    },
  }
}

fn rendered_kind(rendered: &event::RenderedImage) -> &'static str {
  match rendered {
    event::RenderedImage::Symbols { .. } => "symbols",
    event::RenderedImage::Protocol { .. } => "protocol",
  }
}

fn apply_document_reload(
  reload: event::DocumentReload,
  app: &mut App,
  page_store: &mut PageStore,
  renderer: &mut RenderStore,
) {
  let document = reload.document.clone();
  app.apply_document_reload(reload.document, reload.metadata, reload.bookmarks);
  page_store.replace_document(document);
  renderer.clear_state();
}

fn apply_cli_layout(settings: &mut config::Settings, args: &[String]) -> Result<()> {
  if args.is_empty() {
    return Ok(());
  }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileSignature {
  len: u64,
  modified: Option<SystemTime>,
}

fn file_signature(path: &PathBuf) -> Option<FileSignature> {
  let metadata = fs::metadata(path).ok()?;
  Some(FileSignature {
    len: metadata.len(),
    modified: metadata.modified().ok(),
  })
}

fn spawn_input_thread(
  tx: mpsc::UnboundedSender<AsyncEvent>,
  enabled: Arc<AtomicBool>,
  generation: Arc<AtomicU64>,
) {
  thread::spawn(move || {
    loop {
      if !enabled.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(10));
        continue;
      }
      match crossterm_event::read() {
        Ok(event) => {
          let generation = generation.load(Ordering::SeqCst);
          if tx.send(AsyncEvent::Input { event, generation }).is_err() {
            break;
          }
        }
        Err(_) => thread::sleep(Duration::from_millis(10)),
      }
    }
  });
}

fn spawn_auto_refresh_thread(
  tx: mpsc::UnboundedSender<AsyncEvent>,
  path: PathBuf,
  behavior: config::BehaviorConfig,
) {
  if !behavior.auto_refresh {
    return;
  }
  thread::spawn(move || {
    let poll = Duration::from_millis(behavior.auto_refresh_poll_ms.max(200));
    let min_interval = Duration::from_millis(behavior.auto_refresh_min_interval_ms.max(500));
    let mut last_signature = file_signature(&path);
    let mut last_sent = Instant::now()
      .checked_sub(min_interval)
      .unwrap_or_else(Instant::now);
    let mut pending = false;
    loop {
      thread::sleep(poll);
      let signature = file_signature(&path);
      if signature != last_signature {
        last_signature = signature;
        pending = true;
      }
      if pending && last_sent.elapsed() >= min_interval {
        if tx.send(AsyncEvent::AutoRefreshRequested).is_err() {
          break;
        }
        last_sent = Instant::now();
        pending = false;
      }
    }
  });
}

fn discard_pending_terminal_events() {
  while crossterm_event::poll(Duration::from_millis(0)).unwrap_or(false) {
    if crossterm_event::read().is_err() {
      break;
    }
  }
}
