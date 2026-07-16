mod bookmarks_view;
mod footer;
mod grid;
mod metadata_view;
mod modal;
mod page;
mod preload;
mod scroll;
mod search_view;

use img_tui::ProtocolOverlay;
use ratatui::{
  Frame,
  layout::{Constraint, Direction, Rect},
  style::Style,
  widgets::{Block, Paragraph},
};
use tokio::sync::mpsc;
use tracing::debug;

use crate::{
  app::{App, ViewMode},
  event::AsyncEvent,
  pdf::PageStore,
  render::RenderStore,
  terminal::FrameOutput,
};

pub fn draw(
  frame: &mut Frame,
  app: &mut App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
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
  let obscured_areas = completion_overlay.iter().copied().collect::<Vec<_>>();
  let mut overlays = Vec::new();
  let mut cursor_position = None;
  let mut frame_message = None;
  let mut preserve_overlays = false;
  let mut preserve_areas = Vec::new();
  let mut drawn_render_keys = Vec::new();

  draw_main(
    frame,
    app,
    pages,
    renderer,
    tx,
    main,
    &obscured_areas,
    &mut overlays,
    &mut frame_message,
    &mut preserve_overlays,
    &mut preserve_areas,
    &mut drawn_render_keys,
    &mut cursor_position,
  );
  if let Some(area) = completion_overlay {
    footer::draw_command_completion_overlay(frame, app, area);
  }
  footer::draw_footer(
    frame,
    app,
    footer,
    &mut cursor_position,
    frame_message.as_deref(),
  );
  if app.confirm.is_some() {
    modal::draw_confirm(frame, app, area);
  }
  if app.key_help {
    modal::draw_key_help(frame, app, area);
  }
  if app.confirm.is_some() || app.key_help {
    overlays.clear();
    cursor_position = None;
    preserve_overlays = false;
    preserve_areas.clear();
    drawn_render_keys.clear();
  }
  let protocol_writes = if preserve_overlays {
    renderer.take_protocol_writes(&drawn_render_keys, false)
  } else {
    for key in &drawn_render_keys {
      renderer.mark_drawn(key);
    }
    renderer.take_protocol_writes(&drawn_render_keys, true)
  };
  let protocol_write_bytes = protocol_writes.iter().map(String::len).sum::<usize>();
  debug!(
    width = area.width,
    height = area.height,
    main_width = main.width,
    main_height = main.height,
    footer_height,
    overlays = overlays.len(),
    protocol_writes = protocol_writes.len(),
    protocol_write_bytes,
    preserve_overlays,
    "frame output built"
  );

  FrameOutput {
    overlays,
    protocol_writes,
    cursor_position,
    preserve_overlays,
    preserve_areas: if preserve_overlays {
      preserve_areas
    } else {
      Vec::new()
    },
  }
}

fn draw_main(
  frame: &mut Frame,
  app: &mut App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  area: Rect,
  obscured_areas: &[Rect],
  overlays: &mut Vec<ProtocolOverlay>,
  frame_message: &mut Option<String>,
  preserve_overlays: &mut bool,
  preserve_areas: &mut Vec<Rect>,
  drawn_render_keys: &mut Vec<String>,
  cursor_position: &mut Option<(u16, u16)>,
) {
  let theme = &app.settings.theme;
  frame.render_widget(
    Block::default().style(
      Style::default()
        .fg(theme.color(&theme.foreground))
        .bg(theme.color(&theme.background)),
    ),
    area,
  );

  if app.document.page_count == 0 {
    frame.render_widget(Paragraph::new("No pages"), area);
    return;
  }

  if app.view == ViewMode::Metadata {
    metadata_view::draw_metadata(frame, app, area);
    return;
  }

  if app.view == ViewMode::Bookmarks {
    bookmarks_view::draw_bookmarks(
      frame,
      app,
      pages,
      renderer,
      tx,
      area,
      obscured_areas,
      overlays,
      frame_message,
      preserve_overlays,
      preserve_areas,
      drawn_render_keys,
    );
    return;
  }

  if app.view == ViewMode::Search {
    search_view::draw_search(
      frame,
      app,
      pages,
      renderer,
      tx,
      area,
      obscured_areas,
      overlays,
      frame_message,
      preserve_overlays,
      preserve_areas,
      drawn_render_keys,
      cursor_position,
    );
    return;
  }

  if app.layout.is_scroll() {
    scroll::draw_scroll(
      frame,
      app,
      pages,
      renderer,
      tx,
      area,
      obscured_areas,
      overlays,
      frame_message,
      preserve_overlays,
      preserve_areas,
      drawn_render_keys,
    );
  } else {
    grid::draw_grid(
      frame,
      app,
      pages,
      renderer,
      tx,
      area,
      obscured_areas,
      overlays,
      frame_message,
      preserve_overlays,
      preserve_areas,
      drawn_render_keys,
    );
  }
}
