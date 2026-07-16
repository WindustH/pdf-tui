use std::collections::HashSet;

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
  let ahead_rows = row_range_after(last, ahead, scroll_layout.rows.len());
  let behind_rows = row_range_before(first, behind);
  let mut slice_groups = HashSet::new();

  preload_scroll_page_batches(app, pages, tx, area, scroll_layout, &ahead_rows);
  preload_scroll_next_pages(app, pages, tx, area, scroll_layout, visible_rows);
  for index in &ahead_rows {
    preload_scroll_row(
      app,
      pages,
      renderer,
      tx,
      area,
      scroll_layout,
      *index,
      &mut slice_groups,
    );
  }

  preload_scroll_page_batches(app, pages, tx, area, scroll_layout, &behind_rows);
  for index in &behind_rows {
    preload_scroll_row(
      app,
      pages,
      renderer,
      tx,
      area,
      scroll_layout,
      *index,
      &mut slice_groups,
    );
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SlicePreloadGroup {
  page_index: usize,
  slice_count: u16,
  target_width: u32,
  target_height: u32,
  full_cell_width: u16,
  full_cell_height: u16,
}

impl SlicePreloadGroup {
  fn from_spec(spec: crate::pdf::PageSliceSpec) -> Self {
    Self {
      page_index: spec.page_index,
      slice_count: spec.slice_count,
      target_width: spec.target_width,
      target_height: spec.target_height,
      full_cell_width: spec.full_cell_width,
      full_cell_height: spec.full_cell_height,
    }
  }
}

fn row_range_after(last: usize, ahead: usize, row_count: usize) -> Vec<usize> {
  if row_count == 0 {
    return Vec::new();
  }
  let start = last.saturating_add(1);
  if start >= row_count {
    return Vec::new();
  }
  let end = last.saturating_add(ahead).min(row_count.saturating_sub(1));
  (start..=end).collect()
}

fn row_range_before(first: usize, behind: usize) -> Vec<usize> {
  (first.saturating_sub(behind)..first).rev().collect()
}

fn preload_scroll_page_batches(
  app: &App,
  pages: &mut PageStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  area: Rect,
  scroll_layout: &layout::ScrollLayout,
  row_indices: &[usize],
) {
  let mut seen = HashSet::new();
  for row_index in row_indices {
    let Some(row) = scroll_layout.rows.get(*row_index) else {
      continue;
    };
    for item_index in &row.items {
      let Some(item) = scroll_layout.items.get(*item_index).copied() else {
        continue;
      };
      if seen.insert(item.page_index) {
        preload_scroll_page(app, pages, tx, area, item);
      }
    }
  }
}

fn preload_scroll_next_pages(
  app: &App,
  pages: &mut PageStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  area: Rect,
  scroll_layout: &layout::ScrollLayout,
  visible_rows: &[usize],
) {
  let Some(last_visible_page) = max_page_in_rows(scroll_layout, visible_rows) else {
    return;
  };
  let pages_ahead = app.document.pdftoppm_batch_pages.max(1);
  let end = last_visible_page
    .saturating_add(pages_ahead)
    .min(app.document.page_count.saturating_sub(1));
  for page_index in last_visible_page.saturating_add(1)..=end {
    let Some(item) = first_scroll_item_for_page(scroll_layout, page_index) else {
      continue;
    };
    preload_scroll_page(app, pages, tx, area, item);
  }
}

fn max_page_in_rows(scroll_layout: &layout::ScrollLayout, row_indices: &[usize]) -> Option<usize> {
  row_indices
    .iter()
    .filter_map(|row_index| scroll_layout.rows.get(*row_index))
    .flat_map(|row| row.items.iter())
    .filter_map(|item_index| scroll_layout.items.get(*item_index))
    .map(|item| item.page_index)
    .max()
}

fn first_scroll_item_for_page(
  scroll_layout: &layout::ScrollLayout,
  page_index: usize,
) -> Option<layout::ScrollItem> {
  scroll_layout
    .items
    .iter()
    .copied()
    .find(|item| item.page_index == page_index)
}

fn preload_scroll_page(
  app: &App,
  pages: &mut PageStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  area: Rect,
  item: layout::ScrollItem,
) {
  let spec = slice_spec_for_item(app, item, area);
  pages.preload(spec.page_index, spec.target_width, spec.target_height, tx);
}

fn preload_scroll_row(
  app: &App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  area: Rect,
  scroll_layout: &layout::ScrollLayout,
  row_index: usize,
  slice_groups: &mut HashSet<SlicePreloadGroup>,
) {
  let Some(row) = scroll_layout.rows.get(row_index) else {
    return;
  };
  for item_index in &row.items {
    let Some(item) = scroll_layout.items.get(*item_index).copied() else {
      continue;
    };
    let spec = slice_spec_for_item(app, item, area);
    let slice_ready = app.slices.contains_key(&spec);
    let page_ready = app
      .pages
      .get(spec.page_index)
      .and_then(|page| page.as_ref())
      .is_some();
    if page_ready && !slice_ready && slice_groups.insert(SlicePreloadGroup::from_spec(spec)) {
      pages.preload_slice(spec, tx);
    }
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

  for index in last.saturating_add(1)..=last.saturating_add(ahead) {
    if index >= app.document.page_count {
      break;
    }
    preload_grid_page(app, pages, renderer, tx, index, page_area);
  }
  for index in (first.saturating_sub(behind)..first).rev() {
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
