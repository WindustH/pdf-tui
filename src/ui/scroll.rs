use ratatui::{Frame, layout::Rect};
use tracing::debug;

use crate::app::App;

use super::{DrawCtx, page::draw_slice, preload};

pub(super) fn draw_scroll(frame: &mut Frame, app: &mut App, ctx: &mut DrawCtx<'_>, area: Rect) {
  app.prepare_scroll_layout(area);
  let all_ready = draw_visible_slices(frame, app, ctx, area);
  app.finish_frame_render_pass(all_ready);
}

fn draw_visible_slices(frame: &mut Frame, app: &App, ctx: &mut DrawCtx<'_>, area: Rect) -> bool {
  let Some(scroll_layout) = app.scroll_layout() else {
    return true;
  };
  let divisor = app.layout.scroll_divisor;
  let mut all_ready = true;
  let mut visible_summary = Vec::new();
  for placed in scroll_layout.placed_items(app.scroll as usize, area, divisor) {
    let ready = draw_slice(frame, app, ctx, placed.item, placed.area, area);
    all_ready &= ready;
    if tracing::enabled!(tracing::Level::DEBUG) {
      visible_summary.push(format!(
        "p{} s{}/{} row={} y={} h={} ready={ready}",
        placed.item.page_index + 1,
        placed.item.slice_index + 1,
        placed.item.slice_count,
        placed.item.row_index,
        placed.area.y,
        placed.area.height,
      ));
    }
  }
  let visible_rows = scroll_layout.visible_rows(app.scroll as usize, area.height, divisor);
  preload::preload_scroll_neighbors(
    app,
    &mut ctx.preload(),
    area,
    scroll_layout,
    visible_rows.clone(),
  );
  debug!(
    scroll = app.scroll,
    focused_page = app.focused_page + 1,
    viewport_width = area.width,
    viewport_height = area.height,
    total_height = scroll_layout.total_height,
    rows = scroll_layout.rows.len(),
    ?visible_rows,
    visible = ?visible_summary,
    preserve_overlays = ctx.images.preserve_overlays,
    all_ready,
    "scroll draw"
  );
  all_ready
}
