use ratatui::{Frame, layout::Rect};

use crate::{app::App, layout};

use super::{
  DrawCtx,
  page::{draw_page, draw_page_frame_config},
  preload,
};

pub(super) fn draw_grid(frame: &mut Frame, app: &mut App, ctx: &mut DrawCtx<'_>, area: Rect) {
  let slots = layout::grid_slots(area, &app.layout);
  app.set_grid_viewport(area, slots.len().max(1));
  let all_ready = draw_grid_pages(frame, app, ctx, area, &app.layout, app.grid_start_page);
  app.finish_frame_render_pass(all_ready);
}

pub(super) fn draw_grid_pages(
  frame: &mut Frame,
  app: &App,
  ctx: &mut DrawCtx<'_>,
  area: Rect,
  config: &crate::config::EffectiveLayoutConfig,
  start: usize,
) -> bool {
  let slots = layout::grid_slots(area, config);
  let mut all_ready = true;
  let mut visible = Vec::new();
  for (slot_index, slot) in slots.into_iter().enumerate() {
    let page_index = start + slot_index;
    if page_index >= app.document.page_count {
      break;
    }
    visible.push(page_index);
    let page_area = draw_page_frame_config(frame, app, slot, config);
    if config.show_page_numbers && page_area.y > slot.y {
      super::page::draw_page_number(
        frame,
        app,
        page_index,
        Rect::new(page_area.x, page_area.y - 1, page_area.width, 1),
      );
    }
    all_ready &= draw_page(frame, app, ctx, page_index, page_area);
  }
  preload::preload_grid_neighbors(app, &mut ctx.preload(), area, &visible, config);
  all_ready
}
