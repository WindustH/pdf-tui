use img_tui::ProtocolOverlay;
use ratatui::{
  Frame,
  layout::{Constraint, Direction, Rect},
  style::{Modifier, Style},
  text::{Line, Span, Text},
  widgets::{Block, Borders, Paragraph, Wrap},
};
use tokio::sync::mpsc;

use crate::{
  app::App,
  event::AsyncEvent,
  pdf::PageStore,
  render::{RenderKind, RenderStore},
};

use super::page::draw_page;

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_bookmarks(
  frame: &mut Frame,
  app: &mut App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  area: Rect,
  overlays: &mut Vec<ProtocolOverlay>,
  frame_message: &mut Option<String>,
  preserve_overlays: &mut bool,
  preserve_areas: &mut Vec<Rect>,
  drawn_render_keys: &mut Vec<String>,
) {
  app.update_viewport(area);
  let left_ratio = u32::from(app.bookmarks_left_ratio.max(1));
  let right_ratio = u32::from(app.bookmarks_right_ratio.max(1));
  let chunks = ratatui::layout::Layout::default()
    .direction(Direction::Horizontal)
    .constraints([
      Constraint::Ratio(left_ratio, left_ratio.saturating_add(right_ratio)),
      Constraint::Ratio(right_ratio, left_ratio.saturating_add(right_ratio)),
    ])
    .split(area);
  draw_bookmark_tree(frame, app, chunks[0]);
  draw_bookmark_preview(
    frame,
    app,
    pages,
    renderer,
    tx,
    chunks[1],
    overlays,
    frame_message,
    preserve_overlays,
    preserve_areas,
    drawn_render_keys,
  );
}

fn draw_bookmark_tree(frame: &mut Frame, app: &mut App, area: Rect) {
  let inner_height = area.height.saturating_sub(2).max(1);
  app.clamp_bookmarks_scroll(inner_height);
  let theme = &app.settings.theme;
  let base = Style::default()
    .fg(theme.color(&theme.foreground))
    .bg(theme.color(&theme.background));
  let border = Style::default().fg(theme.color(&theme.border));
  let rows = app.visible_bookmark_indices();
  let mut lines = Vec::new();

  if let Some(error) = &app.bookmarks_error {
    lines.push(Line::from(Span::styled(error.clone(), base)));
  } else if rows.is_empty() {
    lines.push(Line::from(Span::styled("No bookmarks", base)));
  } else {
    for index in rows
      .iter()
      .skip(app.bookmarks_scroll as usize)
      .take(inner_height as usize)
    {
      let bookmark = &app.bookmarks[*index];
      let selected = app.bookmarks_selected == Some(*index);
      let has_children = app.bookmark_has_children(*index);
      let marker = if has_children {
        if app.bookmarks_expanded.contains(index) {
          "[-]"
        } else {
          "[+]"
        }
      } else {
        "   "
      };
      let indent = "  ".repeat(bookmark.level.saturating_sub(1) as usize);
      let style = if selected {
        base
          .fg(theme.color(&theme.background))
          .bg(theme.color(&theme.accent))
          .add_modifier(Modifier::BOLD)
      } else {
        base
      };
      lines.push(Line::from(Span::styled(
        format!(
          "{indent}{marker} p{} {}",
          bookmark.page_index + 1,
          bookmark.title
        ),
        style,
      )));
    }
  }

  frame.render_widget(
    Paragraph::new(Text::from(lines))
      .block(
        Block::default()
          .borders(Borders::ALL)
          .title("bookmarks")
          .border_style(border),
      )
      .style(base)
      .wrap(Wrap { trim: false }),
    area,
  );
}

#[allow(clippy::too_many_arguments)]
fn draw_bookmark_preview(
  frame: &mut Frame,
  app: &mut App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  area: Rect,
  overlays: &mut Vec<ProtocolOverlay>,
  frame_message: &mut Option<String>,
  preserve_overlays: &mut bool,
  preserve_areas: &mut Vec<Rect>,
  drawn_render_keys: &mut Vec<String>,
) {
  let theme = &app.settings.theme;
  let base = Style::default()
    .fg(theme.color(&theme.foreground))
    .bg(theme.color(&theme.background));
  let border = Style::default().fg(theme.color(&theme.border));
  frame.render_widget(
    Block::default()
      .borders(Borders::ALL)
      .title("preview")
      .border_style(border)
      .style(base),
    area,
  );
  let inner = super::page::safe_inner(area, 1, 1);
  let Some(bookmark) = app.selected_bookmark() else {
    frame.render_widget(Paragraph::new("No bookmark selected").style(base), inner);
    return;
  };
  let page_index = bookmark
    .page_index
    .min(app.document.page_count.saturating_sub(1));
  draw_page(
    frame,
    app,
    pages,
    renderer,
    tx,
    page_index,
    inner,
    inner.width,
    inner.height,
    RenderKind::Fit,
    overlays,
    frame_message,
    preserve_overlays,
    preserve_areas,
    drawn_render_keys,
  );
}
