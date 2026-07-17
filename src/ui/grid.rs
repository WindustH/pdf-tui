use img_tui::ProtocolOverlay;
use ratatui::{Frame, layout::Rect};
use tokio::sync::mpsc;

use crate::{
  app::App,
  event::AsyncEvent,
  layout,
  pdf::PageStore,
  render::{RenderKind, RenderStore},
};

use super::{
  page::{draw_page, draw_page_frame},
  preload,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_grid(
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
  let slots = layout::grid_slots(area, &app.layout);
  let capacity = slots.len().max(1);
  app.set_grid_viewport(area, capacity);
  let start = app.grid_start_page;
  let mut visible = Vec::new();
  let mut all_ready = true;

  for (slot_index, slot) in slots.into_iter().enumerate() {
    let page_index = start + slot_index;
    if page_index >= app.document.page_count {
      continue;
    }
    visible.push(page_index);
    let page_area = draw_page_frame(frame, app, slot, false);
    let ready = draw_page(
      frame,
      app,
      pages,
      renderer,
      tx,
      page_index,
      page_area,
      page_area.width,
      page_area.height,
      RenderKind::Fit,
      obscured_areas,
      overlays,
      frame_message,
      preserve_overlays,
      preserve_areas,
      drawn_render_keys,
    );
    all_ready &= ready;
  }
  preload::preload_grid_neighbors(app, pages, renderer, tx, area, &visible);
  app.finish_frame_render_pass(all_ready);
}
