use ratatui::{Frame, layout::Rect};

use crate::{app::App, layout};

use super::{
  DrawCtx,
  page::{draw_page, draw_page_frame},
  preload,
};

pub(super) fn draw_grid(frame: &mut Frame, app: &mut App, ctx: &mut DrawCtx<'_>, area: Rect) {
  let slots = layout::grid_slots(area, &app.layout);
  app.set_grid_viewport(area, slots.len().max(1));
  let all_ready = draw_grid_pages(frame, app, ctx, area, slots);
  app.finish_frame_render_pass(all_ready);
}

fn draw_grid_pages(
  frame: &mut Frame,
  app: &App,
  ctx: &mut DrawCtx<'_>,
  area: Rect,
  slots: Vec<Rect>,
) -> bool {
  let start = app.grid_start_page;
  let mut all_ready = true;
  let mut visible = Vec::new();
  for (slot_index, slot) in slots.into_iter().enumerate() {
    let page_index = start + slot_index;
    if page_index >= app.document.page_count {
      break;
    }
    visible.push(page_index);
    let page_area = draw_page_frame(frame, app, slot);
    all_ready &= draw_page(frame, app, ctx, page_index, page_area);
  }
  if app.layout.strategy == "slides" {
    for (page_index, slot) in layout::slide_thumbnails(
      area,
      &app.layout,
      start,
      app.document.page_count,
      |index| app.page_dimensions(index),
      app.terminal_cell_pixels,
    ) {
      let theme = &app.settings.theme;
      let color = if page_index == start {
        &theme.foreground
      } else {
        &theme.border
      };
      let label = format!(
        "{}{}",
        if page_index == start { "> " } else { "" },
        page_index + 1
      );
      let page_area = if app.layout.show_border {
        frame.render_widget(
          ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .title(label)
            .border_style(ratatui::style::Style::default().fg(theme.color(color))),
          slot,
        );
        crate::geometry::slot_content_area(slot, &app.layout)
      } else {
        frame.render_widget(
          ratatui::widgets::Paragraph::new(label)
            .style(ratatui::style::Style::default().fg(theme.color(color))),
          Rect::new(slot.x, slot.y, slot.width, 1),
        );
        crate::geometry::slot_content_area(
          Rect::new(
            slot.x,
            slot.y + 1,
            slot.width,
            slot.height.saturating_sub(1),
          ),
          &app.layout,
        )
      };
      all_ready &= draw_page(frame, app, ctx, page_index, page_area);
    }
  }
  preload::preload_grid_neighbors(app, &mut ctx.preload(), area, &visible);
  all_ready
}
