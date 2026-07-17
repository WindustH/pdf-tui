use img_tui::ProtocolOverlay;
use ratatui::{
  Frame,
  layout::{Constraint, Direction, Rect},
  style::{Color, Modifier, Style},
  text::{Line, Span, Text},
  widgets::{Block, Borders, Paragraph},
};
use tokio::sync::mpsc;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
  obscured_areas: &[Rect],
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
  let preview_ready = draw_bookmark_preview(
    frame,
    app,
    pages,
    renderer,
    tx,
    chunks[1],
    obscured_areas,
    overlays,
    frame_message,
    preserve_overlays,
    preserve_areas,
    drawn_render_keys,
  );
  app.finish_frame_render_pass(preview_ready);
}

fn draw_bookmark_tree(frame: &mut Frame, app: &mut App, area: Rect) {
  let inner_height = area.height.saturating_sub(2).max(1);
  app.clamp_bookmarks_scroll(inner_height);
  let theme = &app.settings.theme;
  let base = Style::default()
    .fg(theme.color(&theme.foreground))
    .bg(theme.color(&theme.background));
  let border = Style::default().fg(theme.color(&theme.border));
  let inner_width = area.width.saturating_sub(2).max(1);
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
      lines.push(bookmark_tree_line(app, *index, inner_width));
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
      .style(base),
    area,
  );
}

fn bookmark_tree_line(app: &App, index: usize, inner_width: u16) -> Line<'static> {
  let theme = &app.settings.theme;
  let bookmark = &app.bookmarks[index];
  let selected = app.bookmarks_selected == Some(index);
  let background = if selected {
    theme.color(&theme.bookmark_hover_background)
  } else {
    theme.color(&theme.background)
  };
  let foreground = if selected {
    theme.color(&theme.bookmark_hover_foreground)
  } else {
    theme.color(&theme.foreground)
  };
  let base = Style::default().fg(foreground).bg(background);
  let title_style = if selected {
    base.add_modifier(Modifier::BOLD)
  } else {
    base
  };
  let spacer_style = Style::default().bg(background);
  let page_foreground = if selected {
    theme.color(&theme.bookmark_hover_page_color)
  } else {
    theme.color(&theme.bookmark_page_color)
  };
  let page_style = Style::default().fg(page_foreground).bg(background);
  let expanded_style = status_style(
    theme.color(&theme.bookmark_expanded_color),
    background,
    selected,
  );
  let collapsed_style = status_style(
    theme.color(&theme.bookmark_collapsed_color),
    background,
    selected,
  );
  let leaf_style = status_style(theme.color(&theme.bookmark_leaf_color), background, false);
  let has_children = app.bookmark_has_children(index);
  let (marker, marker_style) = if has_children {
    if app.bookmarks_expanded.contains(&index) {
      ("[-]", expanded_style)
    } else {
      ("[+]", collapsed_style)
    }
  } else {
    ("   ", leaf_style)
  };
  let page_width = format!("p{}", app.document.page_count.max(1)).width();
  let page = format!("p{}", bookmark.page_index + 1);
  let page = format!("{page:>page_width$}");
  let right_width = page_width + 1 + marker.width();
  let inner_width = usize::from(inner_width);
  let left_width = inner_width.saturating_sub(right_width.saturating_add(1));
  let indent = "  ".repeat(bookmark.level.saturating_sub(1) as usize);
  let title = truncate_to_width(&format!("{indent}{}", bookmark.title), left_width);
  let padding = inner_width.saturating_sub(title.width().saturating_add(right_width));
  Line::from(vec![
    Span::styled(title, title_style),
    Span::styled(" ".repeat(padding), spacer_style),
    Span::styled(page, page_style),
    Span::styled(" ", spacer_style),
    Span::styled(marker.to_string(), marker_style),
  ])
}

fn status_style(foreground: Color, background: Color, bold: bool) -> Style {
  let style = Style::default().fg(foreground).bg(background);
  if bold {
    style.add_modifier(Modifier::BOLD)
  } else {
    style
  }
}

fn truncate_to_width(value: &str, max_width: usize) -> String {
  let mut output = String::new();
  let mut width: usize = 0;
  for ch in value.chars() {
    let ch_width = ch.width().unwrap_or(0);
    if width.saturating_add(ch_width) > max_width {
      break;
    }
    output.push(ch);
    width += ch_width;
  }
  output
}

#[allow(clippy::too_many_arguments)]
fn draw_bookmark_preview(
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
) -> bool {
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
    return true;
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
    obscured_areas,
    overlays,
    frame_message,
    preserve_overlays,
    preserve_areas,
    drawn_render_keys,
  )
}
