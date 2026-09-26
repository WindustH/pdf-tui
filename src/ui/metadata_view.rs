use ratatui::{
  Frame,
  layout::Rect,
  style::{Modifier, Style},
  text::{Line, Span, Text},
  widgets::{Block, Borders, Paragraph},
};
use unicode_width::UnicodeWidthChar;

use crate::app::App;

use super::base_style;

/// Labels are padded to this width so values line up.
const LABEL_WIDTH: usize = 16;

pub(super) fn draw_metadata(frame: &mut Frame, app: &mut App, area: Rect) {
  app.update_viewport(area);
  let theme = &app.settings.theme;
  let base = base_style(app);
  let label_style = base
    .fg(theme.color(&theme.accent))
    .add_modifier(Modifier::BOLD);
  let inner_width = usize::from(area.width.saturating_sub(2)).max(1);
  let rows = metadata_entries(app)
    .iter()
    .flat_map(|(label, value)| wrapped_entry(label, value, inner_width, label_style, base))
    .collect::<Vec<_>>();

  // Rows are wrapped here rather than by the paragraph, so the scroll range
  // covers every row and the last entries stay reachable.
  let inner_height = usize::from(area.height.saturating_sub(2).max(1));
  let max_scroll = u16::try_from(rows.len().saturating_sub(inner_height)).unwrap_or(u16::MAX);
  app.metadata_scroll = app.metadata_scroll.min(max_scroll);
  let visible = rows
    .into_iter()
    .skip(usize::from(app.metadata_scroll))
    .collect::<Vec<_>>();
  let theme = &app.settings.theme;
  frame.render_widget(
    Paragraph::new(Text::from(visible))
      .block(
        Block::default()
          .borders(Borders::ALL)
          .title("metadata")
          .border_style(Style::default().fg(theme.color(&theme.border))),
      )
      .style(base),
    area,
  );
}

/// File facts followed by the tags reported by `exiftool`.
fn metadata_entries(app: &App) -> Vec<(String, String)> {
  let document = &app.document;
  let mut entries = vec![
    ("file".to_string(), document.file_name.clone()),
    ("path".to_string(), document.path.display().to_string()),
    ("pages".to_string(), document.page_count.to_string()),
    ("page size".to_string(), page_size_summary(app)),
    ("dpi".to_string(), document.dpi.to_string()),
  ];
  let summary = if let Some(error) = &app.metadata_error {
    error.clone()
  } else if app.metadata.is_empty() {
    "none".to_string()
  } else {
    format!("{} tags", app.metadata.len())
  };
  entries.push(("metadata".to_string(), summary));
  entries.extend(app.metadata.iter().map(|entry| {
    (
      format!("{}.{}", entry.group, entry.name),
      entry.value.clone(),
    )
  }));
  entries
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

/// One `label value` entry hard-wrapped to `width` cells. Continuation rows
/// are indented to the value column when there is room for it.
fn wrapped_entry(
  label: &str,
  value: &str,
  width: usize,
  label_style: Style,
  value_style: Style,
) -> Vec<Line<'static>> {
  let indent = if width > (LABEL_WIDTH + 1) * 2 {
    LABEL_WIDTH + 1
  } else {
    0
  };
  let label = format!("{label:<LABEL_WIDTH$} ");
  let mut rows = Vec::new();
  let mut row: Vec<Span<'static>> = Vec::new();
  let mut used = 0;
  for (text, style) in [(label.as_str(), label_style), (value, value_style)] {
    let mut chunk = String::new();
    for ch in text.chars() {
      let ch_width = ch.width().unwrap_or(0);
      if used + ch_width > width && used > indent {
        row.push(Span::styled(std::mem::take(&mut chunk), style));
        rows.push(Line::from(std::mem::take(&mut row)));
        row.push(Span::styled(" ".repeat(indent), value_style));
        used = indent;
      }
      chunk.push(ch);
      used += ch_width;
    }
    row.push(Span::styled(chunk, style));
  }
  rows.push(Line::from(row));
  rows
}

#[cfg(test)]
mod tests {
  use super::*;

  fn text(line: &Line<'_>) -> String {
    line
      .spans
      .iter()
      .map(|span| span.content.as_ref())
      .collect()
  }

  #[test]
  fn long_values_wrap_under_the_value_column() {
    let rows = wrapped_entry(
      "path",
      &"x".repeat(60),
      40,
      Style::default(),
      Style::default(),
    );
    let rows = rows.iter().map(text).collect::<Vec<_>>();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0], format!("{:<16} {}", "path", "x".repeat(23)));
    assert_eq!(rows[1], format!("{}{}", " ".repeat(17), "x".repeat(23)));
    assert_eq!(rows[2], format!("{}{}", " ".repeat(17), "x".repeat(14)));
    // Narrow panels wrap without indentation, and every row fits.
    let rows = wrapped_entry("label", "value text", 8, Style::default(), Style::default());
    assert!(rows.iter().all(|row| row.width() <= 8));
  }
}
