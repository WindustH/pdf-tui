use ratatui::layout::Rect;
use tokio::sync::mpsc;

use crate::{
  app::App,
  event::AsyncEvent,
  layout,
  pdf::PageStore,
  render::{RenderKind, RenderStore},
};

use super::page::{page_target_pixels, safe_inner, slice_spec_for_item};

pub(super) fn preload_scroll_neighbors(
  app: &App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  area: Rect,
  scroll_layout: &layout::ScrollLayout,
  visible_rows: &[usize],
) {
  if visible_rows.is_empty() {
    return;
  }
  let first = visible_rows[0];
  let last = *visible_rows.last().unwrap_or(&first);
  let ahead = app.settings.config.render.preload_ahead;
  let behind = app.settings.config.render.preload_behind;
  for index in first.saturating_sub(behind)..first {
    preload_scroll_row(app, pages, renderer, tx, area, scroll_layout, index);
  }
  for index in last.saturating_add(1)..=last.saturating_add(ahead) {
    if index >= scroll_layout.rows.len() {
      break;
    }
    preload_scroll_row(app, pages, renderer, tx, area, scroll_layout, index);
  }
}

fn preload_scroll_row(
  app: &App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  area: Rect,
  scroll_layout: &layout::ScrollLayout,
  row_index: usize,
) {
  let Some(row) = scroll_layout.rows.get(row_index) else {
    return;
  };
  for item_index in &row.items {
    let Some(item) = scroll_layout.items.get(*item_index).copied() else {
      continue;
    };
    let spec = slice_spec_for_item(app, item, area);
    pages.preload_slice(spec, tx);
    if let Some(slice) = app.slices.get(&spec) {
      renderer.preload(slice, item.width, item.height, RenderKind::Fit, tx);
    }
  }
}

pub(super) fn preload_grid_neighbors(
  app: &App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  area: Rect,
  visible: &[usize],
) {
  if visible.is_empty() {
    return;
  }
  let first = visible[0];
  let last = *visible.last().unwrap_or(&first);
  let ahead = app.settings.config.render.preload_ahead;
  let behind = app.settings.config.render.preload_behind;
  let slots = layout::grid_slots(area, &app.layout);
  let Some(slot) = slots.first().copied() else {
    return;
  };
  let page_area = if app.layout.show_border {
    safe_inner(
      slot,
      app.layout.padding.saturating_add(1),
      app.layout.padding.saturating_add(1),
    )
  } else {
    safe_inner(slot, app.layout.padding, app.layout.padding)
  };
  if page_area.width == 0 || page_area.height == 0 {
    return;
  }

  for index in first.saturating_sub(behind)..first {
    preload_grid_page(app, pages, renderer, tx, index, page_area);
  }
  for index in last.saturating_add(1)..=last.saturating_add(ahead) {
    if index >= app.document.page_count {
      break;
    }
    preload_grid_page(app, pages, renderer, tx, index, page_area);
  }
}

fn preload_grid_page(
  app: &App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  index: usize,
  page_area: Rect,
) {
  let (target_width, target_height) = page_target_pixels(
    page_area.width,
    page_area.height,
    app.terminal_cell_pixels,
    app.page_dimensions(index),
  );
  pages.preload(index, target_width, target_height, tx);
  if let Some(page) = app.pages.get(index).and_then(|page| page.as_ref()) {
    renderer.preload(page, page_area.width, page_area.height, RenderKind::Fit, tx);
  }
}
