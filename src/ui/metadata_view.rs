use ratatui::{
  Frame,
  layout::Rect,
  style::{Modifier, Style},
  text::{Line, Span, Text},
  widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::{app::App, config::ThemeConfig};

pub(super) fn draw_metadata(frame: &mut Frame, app: &mut App, area: Rect) {
  app.update_viewport(area);
  let theme = &app.settings.theme;
  let mut lines = vec![
    metadata_line("file", &app.document.file_name, theme),
    metadata_line("path", &app.document.path.display().to_string(), theme),
    metadata_line("pages", &app.document.page_count.to_string(), theme),
    metadata_line("page size", &page_size_summary(app), theme),
    metadata_line("dpi", &app.document.dpi.to_string(), theme),
  ];

  if let Some(error) = &app.metadata_error {
    lines.push(metadata_line("metadata", error, theme));
  } else if app.metadata.is_empty() {
    lines.push(metadata_line("metadata", "none", theme));
  } else {
    lines.push(metadata_line(
      "metadata",
      &format!("{} tags", app.metadata.len()),
      theme,
    ));
    for entry in &app.metadata {
      lines.push(metadata_line(
        &format!("{}.{}", entry.group, entry.name),
        &entry.value,
        theme,
      ));
    }
  }

  let inner_height = area.height.saturating_sub(2).max(1);
  let max_scroll = (lines.len() as u16).saturating_sub(inner_height);
  app.metadata_scroll = app.metadata_scroll.min(max_scroll);
  let visible = lines
    .into_iter()
    .skip(app.metadata_scroll as usize)
    .collect::<Vec<_>>();
  let style = Style::default()
    .fg(theme.color(&theme.foreground))
    .bg(theme.color(&theme.background));
  frame.render_widget(
    Paragraph::new(Text::from(visible))
      .block(
        Block::default()
          .borders(Borders::ALL)
          .title("metadata")
          .border_style(Style::default().fg(theme.color(&theme.border))),
      )
      .style(style)
      .wrap(Wrap { trim: false }),
    area,
  );
}

fn page_size_summary(app: &App) -> String {
  let first = app.document.logical_page_size(0);
  let unique = app
    .document
    .page_sizes
    .iter()
    .copied()
    .collect::<std::collections::BTreeSet<_>>();
  if unique.len() <= 1 {
    return format!("{} x {}", first.0, first.1);
  }
  format!(
    "mixed: {} size(s), first {} x {}",
    unique.len(),
    first.0,
    first.1
  )
}

fn metadata_line(label: &str, value: &str, theme: &ThemeConfig) -> Line<'static> {
  let base = Style::default()
    .fg(theme.color(&theme.foreground))
    .bg(theme.color(&theme.background));
  let label_style = base
    .fg(theme.color(&theme.accent))
    .add_modifier(Modifier::BOLD);
  Line::from(vec![
    Span::styled(format!("{label:<16} "), label_style),
    Span::styled(value.to_string(), base),
  ])
}
