use framework_tui::{PromptLineStyle, draw_prompt_line};
use ratatui::{
  Frame,
  layout::{Constraint, Direction, Rect},
  style::{Modifier, Style},
  text::{Line, Span, Text},
  widgets::Paragraph,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{app::App, geometry::split_panels, overlay::OverlayStep, search::PdfSearchMatch};

use super::{
  DrawCtx, base_style, draw_panel,
  page::{draw_centered, draw_image, fitted_page_request, ready_page},
  page_overlay::overlaid,
  preload,
};

pub(super) fn draw_search(frame: &mut Frame, app: &mut App, ctx: &mut DrawCtx<'_>, area: Rect) {
  app.update_viewport(area);
  let (panel, preview) = split_panels(area, app.search.left_ratio, app.search.right_ratio);
  ctx.cursor_position = draw_search_panel(frame, app, panel);
  let preview_ready = draw_search_preview(frame, app, ctx, preview);
  if app.search_preload_ready() {
    preload::preload_search_previews(app, &mut ctx.preload(), area);
  }
  app.finish_frame_render_pass(preview_ready);
}

fn draw_search_panel(frame: &mut Frame, app: &mut App, area: Rect) -> Option<(u16, u16)> {
  let inner = draw_panel(frame, app, area, "search");
  if inner.height == 0 {
    return None;
  }
  let chunks = ratatui::layout::Layout::default()
    .direction(Direction::Vertical)
    .constraints([Constraint::Length(1), Constraint::Min(0)])
    .split(inner);
  let theme = &app.settings.theme;
  let base = base_style(app);
  let prompt_style = PromptLineStyle {
    base,
    prefix: base.fg(theme.color(&theme.accent)),
    suggestion: base.fg(theme.color(&theme.muted)),
  };
  let cursor = draw_prompt_line(frame, &app.search.prompt, None, chunks[0], &prompt_style);
  draw_search_results(frame, app, chunks[1]);
  cursor
}

fn draw_search_results(frame: &mut Frame, app: &mut App, area: Rect) {
  if area.height == 0 {
    return;
  }
  let visible_height = area.height.max(1);
  app.search.clamp_scroll(visible_height);
  let theme = &app.settings.theme;
  let base = Style::default()
    .fg(theme.color(&theme.foreground))
    .bg(theme.color(&theme.background));
  let muted = base.fg(theme.color(&theme.muted));
  let mut lines = Vec::new();
  let search = &app.search;
  if search.index_loading {
    lines.push(Line::from(Span::styled("Building search index...", muted)));
  } else if let Some(error) = &search.index_error {
    lines.push(Line::from(Span::styled(
      error.clone(),
      base.fg(theme.color(&theme.error)),
    )));
  } else if search.query().is_empty() {
    lines.push(Line::from(Span::styled(
      "Type to search embedded PDF text",
      muted,
    )));
  } else if search.results.is_empty() {
    lines.push(Line::from(Span::styled("No matches", muted)));
  } else {
    let width = area.width as usize;
    for result in search
      .results
      .iter()
      .skip(usize::from(search.scroll))
      .take(visible_height as usize)
    {
      let selected = search.selected == Some(result.id);
      lines.push(search_result_line(app, result, selected, width));
    }
  }
  frame.render_widget(Paragraph::new(Text::from(lines)).style(base), area);
}

fn search_result_line(
  app: &App,
  result: &PdfSearchMatch,
  selected: bool,
  width: usize,
) -> Line<'static> {
  let theme = &app.settings.theme;
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
  let match_style = Style::default()
    .fg(theme.color(&theme.background))
    .bg(theme.color(&theme.accent))
    .add_modifier(Modifier::BOLD);
  let page = format!("p{}", result.page_index + 1);
  let page_width = format!("p{}", app.document.page_count.max(1)).width();
  let page = format!("{page:>page_width$}");
  let page_style = Style::default()
    .fg(if selected {
      theme.color(&theme.bookmark_hover_page_color)
    } else {
      theme.color(&theme.bookmark_page_color)
    })
    .bg(background);
  let right_width = page_width.saturating_add(1);
  let context_width = width.saturating_sub(right_width);
  let mut spans = highlighted_context_spans(result, context_width, base, match_style);
  let used = spans_width(&spans);
  if used < context_width {
    spans.push(Span::styled(" ".repeat(context_width - used), base));
  }
  spans.push(Span::styled(" ", base));
  spans.push(Span::styled(page, page_style));
  Line::from(spans)
}

fn highlighted_context_spans(
  result: &PdfSearchMatch,
  width: usize,
  base: Style,
  match_style: Style,
) -> Vec<Span<'static>> {
  if width == 0 {
    return Vec::new();
  }
  let text = &result.display_text;
  let start = floor_char_boundary(text, result.display_match_start);
  let end = floor_char_boundary(text, result.display_match_end).max(start);
  let window_start = context_window_start(text, start, width);
  let window_end = context_window_end(text, window_start, width);
  let prefix = text_slice(text, window_start, start.min(window_end));
  let matched = text_slice(text, start.max(window_start), end.min(window_end));
  let suffix = text_slice(text, end.max(window_start), window_end);
  let mut spans = Vec::new();
  if window_start > 0 {
    spans.push(Span::styled("...", base));
  }
  if !prefix.is_empty() {
    spans.push(Span::styled(prefix, base));
  }
  if !matched.is_empty() {
    spans.push(Span::styled(matched, match_style));
  }
  if !suffix.is_empty() {
    spans.push(Span::styled(suffix, base));
  }
  if window_end < text.len() {
    spans.push(Span::styled("...", base));
  }
  spans
}

fn draw_search_preview(frame: &mut Frame, app: &App, ctx: &mut DrawCtx<'_>, area: Rect) -> bool {
  let inner = draw_panel(frame, app, area, "preview");
  let Some(result) = app.search.selected_match() else {
    frame.render_widget(
      Paragraph::new("No search result selected").style(base_style(app)),
      inner,
    );
    return true;
  };
  draw_highlighted_page(frame, app, ctx, result, inner)
}

fn draw_highlighted_page(
  frame: &mut Frame,
  app: &App,
  ctx: &mut DrawCtx<'_>,
  result: &PdfSearchMatch,
  area: Rect,
) -> bool {
  if area.width == 0 || area.height == 0 {
    return true;
  }
  let (image_area, (target_width, target_height)) =
    fitted_page_request(app, result.page_index, area);
  if image_area.width == 0 || image_area.height == 0 {
    return true;
  }
  ctx
    .pages
    .request(result.page_index, target_width, target_height, ctx.tx);
  let Some(page) = ready_page(frame, app, ctx, result.page_index, image_area) else {
    return app.page_error(result.page_index).is_some();
  };
  let steps = [OverlayStep::SearchHighlight(result.clone())];
  let (highlighted, highlight_ready) = match overlaid(ctx, page, &steps) {
    Ok(overlaid) => overlaid,
    Err(error) => {
      draw_centered(
        frame,
        image_area,
        format!("search highlight failed\n{error}"),
      );
      return true;
    }
  };
  let drawn = draw_image(frame, ctx, &highlighted, image_area, || {
    format!("drawing highlighted page {}", result.page_index + 1)
  });
  drawn && highlight_ready
}

fn context_window_start(text: &str, match_start: usize, width: usize) -> usize {
  let target = width.saturating_sub(6) / 2;
  let mut used: usize = 0;
  let mut start = match_start;
  for (index, ch) in text[..match_start].char_indices().rev() {
    let ch_width = ch.width().unwrap_or(0);
    if used.saturating_add(ch_width) > target {
      break;
    }
    used += ch_width;
    start = index;
  }
  start
}

fn context_window_end(text: &str, start: usize, width: usize) -> usize {
  let mut used: usize = 0;
  let mut end = start;
  for (offset, ch) in text[start..].char_indices() {
    let ch_width = ch.width().unwrap_or(0);
    if used.saturating_add(ch_width) > width {
      break;
    }
    used += ch_width;
    end = start + offset + ch.len_utf8();
  }
  end
}

/// Largest char boundary of `text` at or below `index`.
fn floor_char_boundary(text: &str, index: usize) -> usize {
  let mut index = index.min(text.len());
  while !text.is_char_boundary(index) {
    index -= 1;
  }
  index
}

fn text_slice(text: &str, start: usize, end: usize) -> String {
  text
    .get(start.min(text.len())..end.min(text.len()))
    .unwrap_or_default()
    .to_string()
}

fn spans_width(spans: &[Span<'_>]) -> usize {
  spans
    .iter()
    .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
    .sum()
}
