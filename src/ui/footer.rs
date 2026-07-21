use framework_tui::{
  CompletionListStyle, KeyHintsStyle, PromptLineStyle, completion_rows,
  default_completion_selected_style, draw_completion_list, draw_key_hints, draw_prompt_line,
  key_hint_columns, key_hint_rows,
};
use ratatui::{
  Frame,
  layout::Rect,
  style::{Modifier, Style},
  text::{Line, Span},
  widgets::{Block, Paragraph},
};

use crate::app::{App, ViewMode};

pub(super) fn footer_height(app: &App, width: u16) -> u16 {
  let status = 1_u16;
  let prompt = u16::from(app.prompt.is_some());
  let hints = app.key_hints();
  let which = if hints.is_empty() {
    0
  } else {
    key_hint_rows(hints.len(), which_key_columns(app, width))
  };
  status.saturating_add(prompt).saturating_add(which)
}

pub(super) fn command_completion_overlay_area(app: &App, area: Rect) -> Option<Rect> {
  let height = command_completion_rows(app).min(area.height);
  if height == 0 || area.width == 0 {
    return None;
  }
  Some(Rect::new(
    area.x,
    area.y.saturating_add(area.height.saturating_sub(height)),
    area.width,
    height,
  ))
}

pub(super) fn draw_command_completion_overlay(frame: &mut Frame, app: &App, area: Rect) {
  draw_command_completion(frame, app, area);
}

pub(super) fn draw_footer(
  frame: &mut Frame,
  app: &App,
  area: Rect,
  cursor_position: &mut Option<(u16, u16)>,
  frame_message: Option<&str>,
) {
  if area.height == 0 {
    return;
  }
  let theme = &app.settings.theme;
  frame.render_widget(
    Block::default().style(
      Style::default()
        .fg(theme.color(&theme.foreground))
        .bg(theme.color(&theme.background)),
    ),
    area,
  );
  let status_area = Rect::new(
    area.x,
    area.y + area.height.saturating_sub(1),
    area.width,
    1,
  );
  let mut content_bottom = area.y.saturating_add(area.height.saturating_sub(1));

  if let Some(prompt) = &app.prompt
    && content_bottom > area.y
  {
    content_bottom = content_bottom.saturating_sub(1);
    let prompt_area = Rect::new(area.x, content_bottom, area.width, 1);
    draw_prompt(frame, app, prompt, prompt_area, cursor_position);
  }

  if !app.key_hints().is_empty() && area.y < content_bottom {
    let which_area = Rect::new(area.x, area.y, area.width, content_bottom - area.y);
    draw_which_key(frame, app, which_area);
  }

  draw_status(frame, app, status_area, frame_message);
}

fn command_completion_rows(app: &App) -> u16 {
  if app.prompt.is_some() {
    completion_rows(app.command_completion(), 5)
  } else {
    0
  }
}

fn which_key_columns(app: &App, width: u16) -> usize {
  key_hint_columns(app.settings.theme.which_key_columns as usize, width)
}

fn draw_prompt(
  frame: &mut Frame,
  app: &App,
  prompt: &framework_tui::Prompt,
  area: Rect,
  cursor_position: &mut Option<(u16, u16)>,
) {
  let theme = &app.settings.theme;
  let base = Style::default()
    .fg(theme.color(&theme.foreground))
    .bg(theme.color(&theme.background));
  let style = PromptLineStyle {
    base,
    prefix: base.fg(theme.color(&theme.accent)),
    suggestion: base.fg(theme.color(&theme.muted)),
  };
  if let Some(position) = draw_prompt_line(frame, prompt, app.command_completion(), area, &style) {
    *cursor_position = Some(position);
  }
}

fn draw_command_completion(frame: &mut Frame, app: &App, area: Rect) {
  let Some(completion) = app.command_completion() else {
    return;
  };
  let theme = &app.settings.theme;
  let base = Style::default()
    .fg(theme.color(&theme.which_key_foreground))
    .bg(theme.color(&theme.which_key_background));
  let style = CompletionListStyle {
    base,
    selected: default_completion_selected_style(),
  };
  draw_completion_list(frame, completion, area, &style);
}

fn draw_which_key(frame: &mut Frame, app: &App, area: Rect) {
  let theme = &app.settings.theme;
  let base = Style::default()
    .fg(theme.color(&theme.which_key_foreground))
    .bg(theme.color(&theme.which_key_background));
  let style = KeyHintsStyle {
    base,
    key: base
      .fg(theme.color(&theme.which_key_key))
      .add_modifier(Modifier::BOLD),
    separator: base.fg(theme.color(&theme.which_key_separator_color)),
    description: base.fg(theme.color(&theme.which_key_description)),
    separator_text: theme.which_key_separator.clone(),
    columns: theme.which_key_columns as usize,
  };
  draw_key_hints(frame, app.key_hints(), area, &style);
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect, frame_message: Option<&str>) {
  let theme = &app.settings.theme;
  let style = Style::default()
    .fg(theme.color(&theme.foreground))
    .bg(theme.color(&theme.background));
  let page = status_page_label(app);
  let view = status_view_label(app);
  frame.render_widget(
    Paragraph::new(Line::from(vec![
      Span::styled("pdf", style.fg(theme.color(&theme.accent))),
      Span::styled(
        format!(
          "  {page}/{}  {}  {}  {}",
          app.document.page_count,
          view,
          app.document.file_name,
          frame_message.unwrap_or(&app.message)
        ),
        style,
      ),
    ]))
    .style(style),
    area,
  );
}

fn status_view_label(app: &App) -> String {
  match app.view {
    ViewMode::Viewer => app.layout.label(),
    ViewMode::Metadata => "metadata".to_string(),
    ViewMode::Bookmarks => "bookmarks".to_string(),
    ViewMode::Search => "search".to_string(),
    ViewMode::Selection => "selection".to_string(),
  }
}

fn status_page_label(app: &App) -> String {
  if app.document.page_count == 0 {
    return "0".to_string();
  }
  if app.view == ViewMode::Selection
    && let Some(selection) = app.current_selection()
  {
    return (selection.page_index + 1).to_string();
  }
  if app.layout.is_scroll() {
    return (app.focused_page + 1).to_string();
  }
  let start = app
    .grid_start_page
    .min(app.document.page_count.saturating_sub(1));
  let end = start
    .saturating_add(app.layout.grid_capacity().max(1))
    .min(app.document.page_count);
  if end <= start + 1 {
    (start + 1).to_string()
  } else {
    format!("{}-{}", start + 1, end)
  }
}
