use framework_tui::{PromptLineStyle, draw_prompt_line};
use img_tui::ProtocolOverlay;
use ratatui::{
  Frame,
  layout::{Constraint, Direction, Rect},
  style::{Modifier, Style},
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
  search::{self, PdfSearchMatch},
};

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_search(
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
  app.update_viewport(area);
  let left_ratio = u32::from(app.search_left_ratio.max(1));
  let right_ratio = u32::from(app.search_right_ratio.max(1));
  let chunks = ratatui::layout::Layout::default()
    .direction(Direction::Horizontal)
    .constraints([
      Constraint::Ratio(left_ratio, left_ratio.saturating_add(right_ratio)),
      Constraint::Ratio(right_ratio, left_ratio.saturating_add(right_ratio)),
    ])
    .split(area);
  draw_search_panel(frame, app, chunks[0], cursor_position);
  let preview_ready = draw_search_preview(
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

fn draw_search_panel(
  frame: &mut Frame,
  app: &mut App,
  area: Rect,
  cursor_position: &mut Option<(u16, u16)>,
) {
  let theme = &app.settings.theme;
  let base = Style::default()
    .fg(theme.color(&theme.foreground))
    .bg(theme.color(&theme.background));
  let border = Style::default().fg(theme.color(&theme.border));
  frame.render_widget(
    Block::default()
      .borders(Borders::ALL)
      .title("search")
      .border_style(border)
      .style(base),
    area,
  );
  let inner = super::page::safe_inner(area, 1, 1);
  if inner.height == 0 {
    return;
  }
  let chunks = ratatui::layout::Layout::default()
    .direction(Direction::Vertical)
    .constraints([Constraint::Length(1), Constraint::Min(0)])
    .split(inner);
  let prompt_style = PromptLineStyle {
    base,
    prefix: base.fg(theme.color(&theme.accent)),
    suggestion: base.fg(theme.color(&theme.muted)),
  };
  *cursor_position = draw_prompt_line(frame, &app.search_prompt, None, chunks[0], &prompt_style);
  draw_search_results(frame, app, chunks[1]);
}

fn draw_search_results(frame: &mut Frame, app: &mut App, area: Rect) {
  if area.height == 0 {
    return;
  }
  let visible_height = area.height.max(1);
  app.clamp_search_scroll(visible_height);
  let theme = &app.settings.theme;
  let base = Style::default()
    .fg(theme.color(&theme.foreground))
    .bg(theme.color(&theme.background));
  let muted = base.fg(theme.color(&theme.muted));
  let mut lines = Vec::new();
  let query = app.search_prompt.buffer().input.trim();
  if app.search_index_loading {
    lines.push(Line::from(Span::styled("Building search index...", muted)));
  } else if let Some(error) = &app.search_index_error {
    lines.push(Line::from(Span::styled(
      error.clone(),
      base.fg(theme.color(&theme.error)),
    )));
  } else if query.is_empty() {
    lines.push(Line::from(Span::styled(
      "Type to search embedded PDF text",
      muted,
    )));
  } else if app.search_results.is_empty() {
    lines.push(Line::from(Span::styled("No matches", muted)));
  } else {
    let width = area.width as usize;
    for result in app
      .search_results
      .iter()
      .skip(app.search_scroll as usize)
      .take(visible_height as usize)
    {
      let selected = app.search_selected == Some(result.id);
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
  let start = result.display_match_start.min(text.len());
  let end = result.display_match_end.min(text.len()).max(start);
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

#[allow(clippy::too_many_arguments)]
fn draw_search_preview(
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
  let Some(result) = app.selected_search_match().cloned() else {
    frame.render_widget(
      Paragraph::new("No search result selected").style(base),
      inner,
    );
    return true;
  };
  draw_highlighted_page(
    frame,
    app,
    pages,
    renderer,
    tx,
    &result,
    inner,
    obscured_areas,
    overlays,
    frame_message,
    preserve_overlays,
    preserve_areas,
    drawn_render_keys,
  )
}

#[allow(clippy::too_many_arguments)]
fn draw_highlighted_page(
  frame: &mut Frame,
  app: &App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  result: &PdfSearchMatch,
  area: Rect,
  obscured_areas: &[Rect],
  overlays: &mut Vec<ProtocolOverlay>,
  frame_message: &mut Option<String>,
  preserve_overlays: &mut bool,
  preserve_areas: &mut Vec<Rect>,
  drawn_render_keys: &mut Vec<String>,
) -> bool {
  if area.width == 0 || area.height == 0 {
    return true;
  }
  let image_area = super::page::fitted_page_area(
    area,
    app.terminal_cell_pixels,
    app.page_dimensions(result.page_index),
  );
  if image_area.width == 0 || image_area.height == 0 {
    return true;
  }
  if super::page::area_intersects_any(image_area, obscured_areas) {
    return true;
  }
  let (target_width, target_height) = super::page::page_target_pixels(
    image_area.width,
    image_area.height,
    app.terminal_cell_pixels,
    app.page_dimensions(result.page_index),
  );
  pages.request(result.page_index, target_width, target_height, tx);
  if let Some(error) = app
    .page_errors
    .get(result.page_index)
    .and_then(|error| error.as_ref())
  {
    super::page::draw_centered(
      frame,
      image_area,
      format!("page {} failed\n{error}", result.page_index + 1),
    );
    return true;
  }
  let Some(page) = app
    .pages
    .get(result.page_index)
    .and_then(|page| page.as_ref())
  else {
    draw_pending(
      frame,
      image_area,
      renderer,
      format!("rendering page {}", result.page_index + 1),
      frame_message,
      preserve_overlays,
      preserve_areas,
    );
    return false;
  };
  let highlighted = match search::highlighted_page_image(
    &app.settings.cache_dir,
    page,
    result,
    app.settings.config.render.search_highlight_cache_max_bytes,
  ) {
    Ok(highlighted) => highlighted,
    Err(error) => {
      super::page::draw_centered(
        frame,
        image_area,
        format!("search highlight failed\n{error}"),
      );
      return true;
    }
  };
  let request = renderer.request(
    &highlighted,
    image_area.width,
    image_area.height,
    RenderKind::Fit,
    tx,
  );
  if let Some(rendered_key) = renderer.rendered_key(&request.cache_key, &request.slot_key, false) {
    if let Some(rendered) = renderer.get(&rendered_key) {
      super::page::draw_rendered_page(frame, image_area, rendered, overlays);
      drawn_render_keys.push(rendered_key);
    }
    true
  } else if let Some(error) = renderer.failure(&request.cache_key) {
    super::page::draw_centered(frame, image_area, format!("render failed\n{error}"));
    true
  } else {
    draw_pending(
      frame,
      image_area,
      renderer,
      format!("drawing highlighted page {}", result.page_index + 1),
      frame_message,
      preserve_overlays,
      preserve_areas,
    );
    false
  }
}

fn draw_pending(
  frame: &mut Frame,
  area: Rect,
  _renderer: &RenderStore,
  text: String,
  _frame_message: &mut Option<String>,
  _preserve_overlays: &mut bool,
  _preserve_areas: &mut Vec<Rect>,
) {
  super::page::draw_centered(frame, area, text);
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
