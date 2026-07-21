use std::collections::HashSet;

use ratatui::layout::{Constraint, Direction, Rect};
use tokio::sync::mpsc;

use crate::{
  app::{App, ViewMode},
  event::AsyncEvent,
  layout,
  pdf::PageStore,
  render::{RenderKind, RenderStore},
  search,
};

use super::page::{fitted_page_area, page_target_pixels, safe_inner, slice_spec_for_item};

pub(super) fn pump_preload(
  app: &App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
) {
  let Some(area) = app.viewport else {
    return;
  };
  match app.view {
    ViewMode::Viewer if app.layout.is_scroll() => {
      let Some(scroll_layout) = app.last_scroll_layout.as_ref() else {
        return;
      };
      let visible_rows = layout::visible_scroll_rows(
        scroll_layout,
        app.scroll as usize,
        area.height,
        app.layout.scroll_divisor,
      );
      preload_scroll_neighbors(app, pages, renderer, tx, area, scroll_layout, &visible_rows);
    }
    ViewMode::Viewer => {
      let capacity = layout::grid_slots(area, &app.layout).len().max(1);
      let start = app.grid_start_page;
      let end = start.saturating_add(capacity).min(app.document.page_count);
      let visible = (start..end).collect::<Vec<_>>();
      preload_grid_neighbors(app, pages, renderer, tx, area, &visible);
    }
    ViewMode::Bookmarks => preload_bookmark_previews(app, pages, renderer, tx, area),
    ViewMode::Search if app.search_preload_ready() => {
      preload_search_previews(app, pages, renderer, tx, area);
    }
    ViewMode::Search | ViewMode::Metadata => {}
  }
}

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
  let slice_ahead = slice_preload_limit(
    ahead,
    app.settings.config.render.preload_slice_ahead,
    app.settings.config.render.preload_terminal_ahead,
  );
  let slice_behind = slice_preload_limit(
    behind,
    app.settings.config.render.preload_slice_behind,
    app.settings.config.render.preload_terminal_behind,
  );
  let terminal_ahead =
    layer_preload_limit(ahead, app.settings.config.render.preload_terminal_ahead);
  let terminal_behind =
    layer_preload_limit(behind, app.settings.config.render.preload_terminal_behind);
  let ahead_rows = row_range_after(last, ahead, scroll_layout.rows.len());
  let behind_rows = row_range_before(first, behind);
  let mut slice_groups = HashSet::new();

  preload_scroll_page_batches(app, pages, tx, area, scroll_layout, &ahead_rows);
  preload_scroll_next_pages(app, pages, tx, area, scroll_layout, visible_rows);
  for (distance, index) in ahead_rows.iter().enumerate() {
    preload_scroll_row(
      app,
      pages,
      renderer,
      tx,
      area,
      scroll_layout,
      *index,
      distance < slice_ahead,
      distance < terminal_ahead,
      &mut slice_groups,
    );
  }

  preload_scroll_page_batches(app, pages, tx, area, scroll_layout, &behind_rows);
  for (distance, index) in behind_rows.iter().enumerate() {
    preload_scroll_row(
      app,
      pages,
      renderer,
      tx,
      area,
      scroll_layout,
      *index,
      distance < slice_behind,
      distance < terminal_behind,
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

fn layer_preload_limit(outer: usize, configured: usize) -> usize {
  configured.min(outer)
}

fn slice_preload_limit(outer: usize, configured: usize, terminal: usize) -> usize {
  configured.max(terminal).min(outer)
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
  let pages_ahead = app.document.pdf_raster_batch_pages.max(1);
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
  preload_slice: bool,
  preload_terminal: bool,
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
    if preload_slice
      && page_ready
      && !slice_ready
      && slice_groups.insert(SlicePreloadGroup::from_spec(spec))
    {
      pages.preload_slice(spec, tx);
    }
    if preload_terminal && let Some(slice) = app.slices.get(&spec) {
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
  let terminal_ahead =
    layer_preload_limit(ahead, app.settings.config.render.preload_terminal_ahead);
  let terminal_behind =
    layer_preload_limit(behind, app.settings.config.render.preload_terminal_behind);
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

  for (distance, index) in (last.saturating_add(1)..=last.saturating_add(ahead)).enumerate() {
    if index >= app.document.page_count {
      break;
    }
    preload_grid_page(
      app,
      pages,
      renderer,
      tx,
      index,
      page_area,
      distance < terminal_ahead,
    );
  }
  for (distance, index) in (first.saturating_sub(behind)..first).rev().enumerate() {
    preload_grid_page(
      app,
      pages,
      renderer,
      tx,
      index,
      page_area,
      distance < terminal_behind,
    );
  }
}

fn preload_grid_page(
  app: &App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  index: usize,
  page_area: Rect,
  preload_terminal: bool,
) {
  let image_area = fitted_page_area(
    page_area,
    app.terminal_cell_pixels,
    app.page_dimensions(index),
  );
  if image_area.width == 0 || image_area.height == 0 {
    return;
  }
  let (target_width, target_height) = page_target_pixels(
    image_area.width,
    image_area.height,
    app.terminal_cell_pixels,
    app.page_dimensions(index),
  );
  pages.preload(index, target_width, target_height, tx);
  if preload_terminal && let Some(page) = app.pages.get(index).and_then(|page| page.as_ref()) {
    renderer.preload(
      page,
      image_area.width,
      image_area.height,
      RenderKind::Fit,
      tx,
    );
  }
}

pub(super) fn preload_bookmark_previews(
  app: &App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  area: Rect,
) {
  let Some(inner) = preview_inner(area, app.bookmarks_left_ratio, app.bookmarks_right_ratio) else {
    return;
  };
  let Some(selected) = app.bookmarks_selected else {
    return;
  };
  let visible = app.visible_bookmark_indices();
  let Some(selected_pos) = visible.iter().position(|index| *index == selected) else {
    return;
  };
  let ahead = app.settings.config.render.preload_ahead;
  let behind = app.settings.config.render.preload_behind;
  let terminal_ahead =
    layer_preload_limit(ahead, app.settings.config.render.preload_terminal_ahead);
  let terminal_behind =
    layer_preload_limit(behind, app.settings.config.render.preload_terminal_behind);
  let start = selected_pos.saturating_sub(behind);
  let end = selected_pos
    .saturating_add(ahead)
    .min(visible.len().saturating_sub(1));
  let mut seen_pages = HashSet::new();
  for pos in start..=end {
    let Some(bookmark) = visible.get(pos).and_then(|index| app.bookmarks.get(*index)) else {
      continue;
    };
    let page_index = bookmark
      .page_index
      .min(app.document.page_count.saturating_sub(1));
    if !seen_pages.insert(page_index) {
      continue;
    }
    let distance = pos.abs_diff(selected_pos);
    let preload_terminal = if pos >= selected_pos {
      distance <= terminal_ahead
    } else {
      distance <= terminal_behind
    };
    preload_page_preview(
      app,
      pages,
      renderer,
      tx,
      page_index,
      inner,
      preload_terminal,
    );
  }
}

pub(super) fn preload_search_previews(
  app: &App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  area: Rect,
) {
  let query = app.search_prompt.buffer().input.trim();
  if query.is_empty() || app.search_results.is_empty() {
    return;
  }
  let Some(inner) = preview_inner(area, app.search_left_ratio, app.search_right_ratio) else {
    return;
  };
  let Some(selected) = app.search_selected else {
    return;
  };
  let ahead = app.settings.config.render.preload_ahead;
  let behind = app.settings.config.render.preload_behind;
  let terminal_ahead =
    layer_preload_limit(ahead, app.settings.config.render.preload_terminal_ahead);
  let terminal_behind =
    layer_preload_limit(behind, app.settings.config.render.preload_terminal_behind);
  let start = selected.saturating_sub(behind);
  let end = selected
    .saturating_add(ahead)
    .min(app.search_results.len().saturating_sub(1));

  for index in start..=end {
    let Some(result) = app.search_results.get(index) else {
      continue;
    };
    let distance = index.abs_diff(selected);
    let preload_terminal = if index >= selected {
      distance <= terminal_ahead
    } else {
      distance <= terminal_behind
    };
    preload_search_preview(app, pages, renderer, tx, result, inner, preload_terminal);
  }
}

fn preload_search_preview(
  app: &App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  result: &search::PdfSearchMatch,
  area: Rect,
  preload_terminal: bool,
) {
  let image_area = fitted_page_area(
    area,
    app.terminal_cell_pixels,
    app.page_dimensions(result.page_index),
  );
  if image_area.width == 0 || image_area.height == 0 {
    return;
  }
  let (target_width, target_height) = page_target_pixels(
    image_area.width,
    image_area.height,
    app.terminal_cell_pixels,
    app.page_dimensions(result.page_index),
  );
  pages.preload(result.page_index, target_width, target_height, tx);
  if !preload_terminal {
    return;
  }
  let Some(page) = app
    .pages
    .get(result.page_index)
    .and_then(|page| page.as_ref())
  else {
    return;
  };
  let Ok(highlighted) = search::highlighted_page_image(
    &app.settings.cache_dir,
    page,
    result,
    app.settings.config.render.search_highlight_cache_max_bytes,
  ) else {
    return;
  };
  renderer.preload(
    &highlighted,
    image_area.width,
    image_area.height,
    RenderKind::Fit,
    tx,
  );
}

fn preload_page_preview(
  app: &App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  page_index: usize,
  area: Rect,
  preload_terminal: bool,
) {
  let image_area = fitted_page_area(
    area,
    app.terminal_cell_pixels,
    app.page_dimensions(page_index),
  );
  if image_area.width == 0 || image_area.height == 0 {
    return;
  }
  let (target_width, target_height) = page_target_pixels(
    image_area.width,
    image_area.height,
    app.terminal_cell_pixels,
    app.page_dimensions(page_index),
  );
  pages.preload(page_index, target_width, target_height, tx);
  if preload_terminal && let Some(page) = app.pages.get(page_index).and_then(|page| page.as_ref()) {
    renderer.preload(
      page,
      image_area.width,
      image_area.height,
      RenderKind::Fit,
      tx,
    );
  }
}

fn preview_inner(area: Rect, left_ratio: u16, right_ratio: u16) -> Option<Rect> {
  let left_ratio = u32::from(left_ratio.max(1));
  let right_ratio = u32::from(right_ratio.max(1));
  let chunks = ratatui::layout::Layout::default()
    .direction(Direction::Horizontal)
    .constraints([
      Constraint::Ratio(left_ratio, left_ratio.saturating_add(right_ratio)),
      Constraint::Ratio(right_ratio, left_ratio.saturating_add(right_ratio)),
    ])
    .split(area);
  let preview = chunks.get(1).copied()?;
  Some(safe_inner(preview, 1, 1))
}
