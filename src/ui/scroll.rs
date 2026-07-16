use img_tui::ProtocolOverlay;
use ratatui::{Frame, layout::Rect};
use tokio::sync::mpsc;
use tracing::debug;

use crate::{app::App, event::AsyncEvent, layout, pdf::PageStore, render::RenderStore};

use super::{page::draw_slice, preload};

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_scroll(
  frame: &mut Frame,
  app: &mut App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  area: Rect,
  overlays: &mut Vec<ProtocolOverlay>,
  frame_message: &mut Option<String>,
  preserve_overlays: &mut bool,
  preserve_areas: &mut Vec<Rect>,
  drawn_render_keys: &mut Vec<String>,
) {
  let scroll_layout = layout::compute_scroll_layout(
    app.document.page_count,
    area.width,
    area.height,
    &app.layout,
    |index| app.page_dimensions(index),
    app.terminal_cell_pixels,
  );
  app.update_scroll_layout(scroll_layout.clone(), area);

  let visible_rows = layout::visible_scroll_rows(
    &scroll_layout,
    app.scroll as usize,
    area.height,
    app.layout.scroll_divisor,
  );
  let used_height = layout::visible_rows_height(&scroll_layout, &visible_rows);
  let mut row_y = area
    .y
    .saturating_add(area.height.saturating_sub(used_height) / 2);
  let mut visible_summary = Vec::new();
  for (row_position, row_index) in visible_rows.iter().copied().enumerate() {
    let Some(row) = scroll_layout.rows.get(row_index) else {
      continue;
    };
    for item_index in &row.items {
      let Some(item) = scroll_layout.items.get(*item_index).copied() else {
        continue;
      };
      let item_y = row_y.saturating_add(row.height.saturating_sub(item.height) / 2);
      let item_area = Rect::new(
        area.x.saturating_add(item.x),
        item_y,
        item.width.min(area.width.saturating_sub(item.x)),
        item.height,
      );
      let ready = draw_slice(
        frame,
        app,
        pages,
        renderer,
        tx,
        item,
        item_area,
        area,
        overlays,
        frame_message,
        preserve_overlays,
        preserve_areas,
        drawn_render_keys,
      );
      visible_summary.push(format!(
        "p{} s{}/{} row={} y={} h={} ready={}",
        item.page_index + 1,
        item.slice_index + 1,
        item.slice_count,
        row_index,
        item_area.y,
        item_area.height,
        ready
      ));
    }
    row_y = row_y.saturating_add(row.height);
    if row_position + 1 < visible_rows.len() {
      row_y = row_y.saturating_add(row.gap_after);
    }
  }
  preload::preload_scroll_neighbors(
    app,
    pages,
    renderer,
    tx,
    area,
    &scroll_layout,
    &visible_rows,
  );
  debug!(
    scroll = app.scroll,
    focused_page = app.focused_page + 1,
    viewport_width = area.width,
    viewport_height = area.height,
    total_height = scroll_layout.total_height,
    rows = scroll_layout.rows.len(),
    visible_rows = ?visible_rows,
    visible = ?visible_summary,
    preserve_overlays = *preserve_overlays,
    "scroll draw"
  );
}
