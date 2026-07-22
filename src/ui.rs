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

#[derive(Default)]
struct UiFrameState {
  overlays: Vec<ProtocolOverlay>,
  cursor_position: Option<(u16, u16)>,
  frame_message: Option<String>,
  preserve_overlays: bool,
  preserve_areas: Vec<Rect>,
  drawn_render_keys: Vec<String>,
}

impl UiFrameState {
  fn clear_transient_output(&mut self) {
    self.overlays.clear();
    self.cursor_position = None;
    self.preserve_overlays = false;
    self.preserve_areas.clear();
    self.drawn_render_keys.clear();
  }
}

struct UiDrawContext<'a> {
  app: &'a mut App,
  pages: &'a mut PageStore,
  renderer: &'a mut RenderStore,
  tx: &'a mpsc::UnboundedSender<AsyncEvent>,
  obscured_areas: &'a [Rect],
  state: &'a mut UiFrameState,
}

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
  let mut state = UiFrameState::default();

  {
    let mut ctx = UiDrawContext {
      app,
      pages,
      renderer,
      tx,
      obscured_areas: &obscured_areas,
      state: &mut state,
    };
    draw_main(frame, main, &mut ctx);
  }
  if let Some(area) = completion_overlay {
    footer::draw_command_completion_overlay(frame, app, area);
  }
  footer::draw_footer(
    frame,
    app,
    footer,
    &mut state.cursor_position,
    state.frame_message.as_deref(),
  );
  if app.confirm.is_some() {
    modal::draw_confirm(frame, app, area);
  }
  if app.key_help {
    modal::draw_key_help(frame, app, area);
  }
  if app.confirm.is_some() || app.key_help {
    state.clear_transient_output();
  }
  let protocol_writes = if state.preserve_overlays {
    renderer.take_protocol_writes(&state.drawn_render_keys, false)
  } else {
    for key in &state.drawn_render_keys {
      renderer.mark_drawn(key);
    }
    renderer.take_protocol_writes(&state.drawn_render_keys, true)
  };
  let protocol_write_bytes = protocol_writes.iter().map(String::len).sum::<usize>();
  debug!(
    width = area.width,
    height = area.height,
    main_width = main.width,
    main_height = main.height,
    footer_height,
    overlays = state.overlays.len(),
    protocol_writes = protocol_writes.len(),
    protocol_write_bytes,
    preserve_overlays = state.preserve_overlays,
    "frame output built"
  );

  let preserve_overlays = state.preserve_overlays;
  let preserve_areas = if preserve_overlays {
    state.preserve_areas
  } else {
    Vec::new()
  };
  FrameOutput {
    overlays: state.overlays,
    protocol_writes,
    cursor_position: state.cursor_position,
    preserve_overlays,
    preserve_areas,
  }
}

pub fn pump_preload(
  app: &mut App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
) {
  preload::pump_preload(app, pages, renderer, tx);
}

fn draw_main(frame: &mut Frame, area: Rect, ctx: &mut UiDrawContext<'_>) {
  let theme = &ctx.app.settings.theme;
  frame.render_widget(
    Block::default().style(
      Style::default()
        .fg(theme.color(&theme.foreground))
        .bg(theme.color(&theme.background)),
    ),
    area,
  );

  if ctx.app.document.page_count == 0 {
    frame.render_widget(Paragraph::new("No pages"), area);
    return;
  }

  if ctx.app.view == ViewMode::Metadata {
    metadata_view::draw_metadata(frame, ctx.app, area);
    return;
  }

  if ctx.app.view == ViewMode::Bookmarks {
    bookmarks_view::draw_bookmarks(
      frame,
      ctx.app,
      ctx.pages,
      ctx.renderer,
      ctx.tx,
      area,
      ctx.obscured_areas,
      &mut ctx.state.overlays,
      &mut ctx.state.frame_message,
      &mut ctx.state.preserve_overlays,
      &mut ctx.state.preserve_areas,
      &mut ctx.state.drawn_render_keys,
    );
    return;
  }

  if ctx.app.view == ViewMode::Search {
    search_view::draw_search(
      frame,
      ctx.app,
      ctx.pages,
      ctx.renderer,
      ctx.tx,
      area,
      ctx.obscured_areas,
      &mut ctx.state.overlays,
      &mut ctx.state.frame_message,
      &mut ctx.state.preserve_overlays,
      &mut ctx.state.preserve_areas,
      &mut ctx.state.drawn_render_keys,
      &mut ctx.state.cursor_position,
    );
    return;
  }

  if ctx.app.view == ViewMode::Selection {
    selection_view::draw_selection(
      frame,
      ctx.app,
      ctx.pages,
      ctx.renderer,
      ctx.tx,
      area,
      ctx.obscured_areas,
      &mut ctx.state.overlays,
      &mut ctx.state.frame_message,
      &mut ctx.state.preserve_overlays,
      &mut ctx.state.preserve_areas,
      &mut ctx.state.drawn_render_keys,
    );
    return;
  }

  if ctx.app.layout.is_scroll() {
    scroll::draw_scroll(
      frame,
      ctx.app,
      ctx.pages,
      ctx.renderer,
      ctx.tx,
      area,
      ctx.obscured_areas,
      &mut ctx.state.overlays,
      &mut ctx.state.frame_message,
      &mut ctx.state.preserve_overlays,
      &mut ctx.state.preserve_areas,
      &mut ctx.state.drawn_render_keys,
    );
  } else {
    grid::draw_grid(
      frame,
      ctx.app,
      ctx.pages,
      ctx.renderer,
      ctx.tx,
      area,
      ctx.obscured_areas,
      &mut ctx.state.overlays,
      &mut ctx.state.frame_message,
      &mut ctx.state.preserve_overlays,
      &mut ctx.state.preserve_areas,
      &mut ctx.state.drawn_render_keys,
    );
  }
}
