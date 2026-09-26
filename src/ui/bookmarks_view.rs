use ratatui::{
  Frame,
  layout::Rect,
  style::{Color, Modifier, Style},
  text::{Line, Span, Text},
  widgets::{Block, Borders, Paragraph},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{app::App, geometry::split_panels};

use super::{DrawCtx, draw_panel, page::draw_page, preload};

pub(super) fn draw_bookmarks(frame: &mut Frame, app: &mut App, ctx: &mut DrawCtx<'_>, area: Rect) {
  app.update_viewport(area);
  let (tree, preview) = split_panels(area, app.bookmarks.left_ratio, app.bookmarks.right_ratio);
  draw_bookmark_tree(frame, app, tree);
  let preview_ready = draw_bookmark_preview(frame, app, ctx, preview);
  preload::preload_bookmark_previews(app, &mut ctx.preload(), area);
  app.finish_frame_render_pass(preview_ready);
}

fn draw_bookmark_tree(frame: &mut Frame, app: &mut App, area: Rect) {
  let inner_height = area.height.saturating_sub(2).max(1);
  app.bookmarks.clamp_scroll(inner_height);
  let theme = &app.settings.theme;
  let base = Style::default()
    .fg(theme.color(&theme.foreground))
    .bg(theme.color(&theme.background));
  let border = Style::default().fg(theme.color(&theme.border));
  let inner_width = area.width.saturating_sub(2).max(1);
  let rows = app.bookmarks.visible_indices();
  let mut lines = Vec::new();

  if let Some(error) = &app.bookmarks.error {
    lines.push(Line::from(Span::styled(error.clone(), base)));
  } else if rows.is_empty() {
    lines.push(Line::from(Span::styled("No bookmarks", base)));
  } else {
    for index in rows
      .iter()
      .skip(usize::from(app.bookmarks.scroll))
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
  let bookmark = &app.bookmarks.entries[index];
  let selected = app.bookmarks.selected == Some(index);
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
  let has_children = app.bookmarks.has_children(index);
  let (marker, marker_style) = if has_children {
    if app.bookmarks.expanded.contains(&index) {
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

fn draw_bookmark_preview(frame: &mut Frame, app: &App, ctx: &mut DrawCtx<'_>, area: Rect) -> bool {
  let inner = draw_panel(frame, app, area, "preview");
  let Some(bookmark) = app.bookmarks.selected_entry() else {
    frame.render_widget(
      Paragraph::new("No bookmark selected").style(super::base_style(app)),
      inner,
    );
    return true;
  };
  let page_index = bookmark
    .page_index
    .min(app.document.page_count.saturating_sub(1));
  draw_page(frame, app, ctx, page_index, inner)
}
