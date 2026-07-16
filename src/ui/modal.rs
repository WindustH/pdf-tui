use framework_tui::{
  KeyHelpDialogStyle, PopupDialogStyle, draw_key_help_dialog, draw_popup_dialog,
};
use ratatui::{
  Frame,
  layout::Rect,
  style::{Modifier, Style},
  text::{Line, Span, Text},
};

use crate::app::{App, ConfirmDialog};

pub(super) fn draw_confirm(frame: &mut Frame, app: &App, area: Rect) {
  let Some(confirm) = &app.confirm else {
    return;
  };
  let theme = &app.settings.theme;
  let style = Style::default()
    .fg(theme.color(&theme.foreground))
    .bg(theme.color(&theme.which_key_background));
  let text = match confirm {
    ConfirmDialog::MetadataWrite { edit } => {
      let mut lines = vec![
        Line::from(Span::styled(
          "Apply PDF metadata changes?",
          style
            .fg(theme.color(&theme.accent))
            .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
          format!(
            "{} change(s): {}",
            edit.change_count(),
            app.document.file_name
          ),
          style,
        )),
      ];
      for change in edit.tags.iter().take(5) {
        lines.push(Line::from(Span::styled(
          format!("{}: {}", change.tag, change.new_value),
          style,
        )));
      }
      if edit.tags.len() > 5 {
        lines.push(Line::from(Span::styled("...", style)));
      }
      lines.push(Line::from(Span::styled(
        "y apply    Enter/n/esc cancel",
        style.fg(theme.color(&theme.muted)),
      )));
      Text::from(lines)
    }
  };
  let popup_style = PopupDialogStyle {
    base: style,
    border: style,
    ..PopupDialogStyle::default()
  };
  let _ = draw_popup_dialog(frame, area, "confirm", text, &popup_style);
}

pub(super) fn draw_key_help(frame: &mut Frame, app: &App, area: Rect) {
  if !app.key_help {
    return;
  }
  let theme = &app.settings.theme;
  let style = Style::default()
    .fg(theme.color(&theme.foreground))
    .bg(theme.color(&theme.which_key_background));
  let key_style = style
    .fg(theme.color(&theme.which_key_key))
    .add_modifier(Modifier::BOLD);
  let desc_style = style.fg(theme.color(&theme.which_key_description));
  let muted = style.fg(theme.color(&theme.muted));
  let entries = app.key_help_entries();
  let help_style = KeyHelpDialogStyle {
    popup: PopupDialogStyle {
      base: style,
      border: style,
      max_height: area.height.saturating_sub(2).clamp(8, 34),
      ..PopupDialogStyle::default()
    },
    key: key_style,
    description: desc_style,
    muted,
    ..KeyHelpDialogStyle::default()
  };
  let _ = draw_key_help_dialog(frame, area, app.key_help_title(), &entries, &help_style);
}
