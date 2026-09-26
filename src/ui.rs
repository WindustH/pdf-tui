mod bookmarks_view;
mod footer;
mod grid;
mod metadata_view;
mod modal;
mod page;
mod page_overlay;
mod preload;
mod scroll;
mod search_view;
mod selection_view;

use img_tui::ProtocolOverlay;
use ratatui::{
  Frame,
  layout::{Constraint, Direction, Rect},
  style::Style,
  widgets::{Block, Borders, Paragraph},
};
use tokio::sync::mpsc;
use tracing::debug;

use crate::{
  app::{App, ViewMode},
  event::AsyncEvent,
  geometry::safe_inner,
  overlay::OverlayStore,
  pdf::PageStore,
  render::RenderStore,
  terminal::FrameOutput,
};

pub use preload::pump_preload;

/// Protocol image output collected while drawing one frame.
#[derive(Default)]
struct FrameImages {
  overlays: Vec<ProtocolOverlay>,
  /// First "still rendering" message of the frame, shown in the status line.
  pending_message: Option<String>,
  /// Set when a protocol image is still pending: the previous frame's
  /// images stay on screen instead of flashing to blank.
  preserve_overlays: bool,
  preserve_areas: Vec<Rect>,
  drawn_render_keys: Vec<String>,
}

/// The stages that turn PDF pages into terminal output, each with its own
/// job scheduling and caches: page/slice PNGs, marked-up copies of them,
/// and terminal renders.
pub struct ImagePipeline {
  pub pages: PageStore,
  pub overlays: OverlayStore,
  pub renderer: RenderStore,
}

/// What the views need to request and draw page images.
struct DrawCtx<'a> {
  pages: &'a mut PageStore,
  overlays: &'a mut OverlayStore,
  renderer: &'a mut RenderStore,
  tx: &'a mpsc::UnboundedSender<AsyncEvent>,
  images: FrameImages,
  cursor_position: Option<(u16, u16)>,
}

impl DrawCtx<'_> {
  fn preload(&mut self) -> preload::PreloadCtx<'_> {
    preload::PreloadCtx {
      pages: self.pages,
      overlays: self.overlays,
      renderer: self.renderer,
      tx: self.tx,
    }
  }
}

pub fn draw(
  frame: &mut Frame,
  app: &mut App,
  pipeline: &mut ImagePipeline,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
) -> FrameOutput {
  let area = frame.area();
  let footer_height = footer::footer_height(app, area.width).min(area.height);
  let chunks = ratatui::layout::Layout::default()
    .direction(Direction::Vertical)
    .constraints([Constraint::Min(1), Constraint::Length(footer_height)])
    .split(area);
  let main = chunks[0];
  let footer = chunks[1];
  let completion_overlay = footer::command_completion_overlay_area(app, main);

  let mut ctx = DrawCtx {
    pages: &mut pipeline.pages,
    overlays: &mut pipeline.overlays,
    renderer: &mut pipeline.renderer,
    tx,
    images: FrameImages::default(),
    cursor_position: None,
  };
  draw_main(frame, main, app, &mut ctx);
  let DrawCtx {
    images,
    mut cursor_position,
    ..
  } = ctx;
  let renderer = &mut pipeline.renderer;

  if let Some(area) = completion_overlay {
    footer::draw_command_completion_overlay(frame, app, area);
  }
  footer::draw_footer(
    frame,
    app,
    footer,
    &mut cursor_position,
    images.pending_message.as_deref(),
  );
  // Text-cell modals do not remove protocol overlays: their rects become
  // occluders that replace kitty U=1 placeholder cells. Uncovered cells keep
  // showing the page, and the regular text diff restores placeholders when
  // a modal closes.
  let mut occluders = Vec::new();
  occluders.extend(completion_overlay);
  occluders.extend(modal::draw_confirm(frame, app, area));
  occluders.extend(modal::draw_key_help(frame, app, area));
  if app.confirm.is_some() || app.key_help {
    cursor_position = None;
  }
  if !images.preserve_overlays {
    for key in &images.drawn_render_keys {
      renderer.mark_drawn(key);
    }
  }
  debug!(
    width = area.width,
    height = area.height,
    main_width = main.width,
    main_height = main.height,
    footer_height,
    overlays = images.overlays.len(),
    preserve_overlays = images.preserve_overlays,
    "frame output built"
  );

  let preserve_areas = if images.preserve_overlays {
    images.preserve_areas
  } else {
    Vec::new()
  };
  FrameOutput {
    overlays: images.overlays,
    protocol_writes: Vec::new(),
    cursor_position,
    preserve_overlays: images.preserve_overlays,
    preserve_areas,
    occluders,
  }
}

fn draw_main(frame: &mut Frame, area: Rect, app: &mut App, ctx: &mut DrawCtx<'_>) {
  frame.render_widget(Block::default().style(base_style(app)), area);

  if app.document.page_count == 0 {
    frame.render_widget(Paragraph::new("No pages"), area);
    return;
  }

  match app.view {
    ViewMode::Metadata => metadata_view::draw_metadata(frame, app, area),
    ViewMode::Bookmarks => bookmarks_view::draw_bookmarks(frame, app, ctx, area),
    ViewMode::Search => search_view::draw_search(frame, app, ctx, area),
    ViewMode::Selection => selection_view::draw_selection(frame, app, ctx, area),
    ViewMode::Viewer if app.layout.is_scroll() => scroll::draw_scroll(frame, app, ctx, area),
    ViewMode::Viewer => grid::draw_grid(frame, app, ctx, area),
  }
}

/// Foreground and background of plain text in the theme.
fn base_style(app: &App) -> Style {
  let theme = &app.settings.theme;
  Style::default()
    .fg(theme.color(&theme.foreground))
    .bg(theme.color(&theme.background))
}

/// Draws a bordered, titled panel and returns the area inside the border.
fn draw_panel(frame: &mut Frame, app: &App, area: Rect, title: &str) -> Rect {
  let theme = &app.settings.theme;
  frame.render_widget(
    Block::default()
      .borders(Borders::ALL)
      .title(title)
      .border_style(Style::default().fg(theme.color(&theme.border)))
      .style(base_style(app)),
    area,
  );
  safe_inner(area, 1, 1)
}
