//! Warming caches around what is visible: page PNGs farthest out, scroll
//! slices nearer, and terminal renders for the nearest neighbors.

use std::{collections::HashSet, ops::Range};

use ratatui::layout::Rect;
use tokio::sync::mpsc;

use crate::{
  app::{App, ViewMode},
  config::RenderConfig,
  event::AsyncEvent,
  geometry::{fitted_page_area, safe_inner, slot_content_area, split_panels},
  layout::{self, ScrollItem, ScrollLayout},
  overlay::{OverlayState, OverlayStep, OverlayStore},
  pdf::{PageImage, PageSliceSpec, PageStore},
  render::{RenderKind, RenderStore},
  search, selection,
};

use super::{
  ImagePipeline,
  page::{fitted_page_request, slice_spec_for_item},
};

pub(super) struct PreloadCtx<'a> {
  pub(super) pages: &'a mut PageStore,
  pub(super) overlays: &'a mut OverlayStore,
  pub(super) renderer: &'a mut RenderStore,
  pub(super) tx: &'a mpsc::UnboundedSender<AsyncEvent>,
}

impl PreloadCtx<'_> {
  fn preload_terminal(&mut self, image: &PageImage, area: Rect) {
    self
      .renderer
      .preload(image, area.width, area.height, RenderKind::Fit, self.tx);
  }
}

/// Distances around the visible region for each cache layer, each capped
/// by the outer page window.
struct PreloadLimits {
  ahead: usize,
  behind: usize,
  slice_ahead: usize,
  slice_behind: usize,
  terminal_ahead: usize,
  terminal_behind: usize,
}

impl PreloadLimits {
  fn new(render: &RenderConfig) -> Self {
    let ahead = render.preload_ahead;
    let behind = render.preload_behind;
    Self {
      ahead,
      behind,
      // Terminal renders need their slice, so the slice window covers it.
      slice_ahead: render
        .preload_slice_ahead
        .max(render.preload_terminal_ahead)
        .min(ahead),
      slice_behind: render
        .preload_slice_behind
        .max(render.preload_terminal_behind)
        .min(behind),
      terminal_ahead: render.preload_terminal_ahead.min(ahead),
      terminal_behind: render.preload_terminal_behind.min(behind),
    }
  }

  /// Indices of a list within the preload window around `selected`, each
  /// with whether it is close enough for a terminal render.
  fn list_window(&self, selected: usize, len: usize) -> impl Iterator<Item = (usize, bool)> + '_ {
    let start = selected.saturating_sub(self.behind);
    let end = selected
      .saturating_add(self.ahead)
      .min(len.saturating_sub(1));
    (start..=end).filter(move |_| len > 0).map(move |index| {
      let terminal = if index >= selected {
        index - selected <= self.terminal_ahead
      } else {
        selected - index <= self.terminal_behind
      };
      (index, terminal)
    })
  }
}

/// Queues preloads around the current position, e.g. after a page render
/// finished; drawing a frame queues them as well.
pub fn pump_preload(
  app: &mut App,
  pipeline: &mut ImagePipeline,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
) {
  let Some(area) = app.viewport else {
    return;
  };
  let ctx = &mut PreloadCtx {
    pages: &mut pipeline.pages,
    overlays: &mut pipeline.overlays,
    renderer: &mut pipeline.renderer,
    tx,
  };
  match app.view {
    ViewMode::Viewer if app.layout.is_scroll() => {
      let Some(scroll_layout) = app.scroll_layout() else {
        return;
      };
      let visible_rows =
        scroll_layout.visible_rows(app.scroll as usize, area.height, app.layout.scroll_divisor);
      preload_scroll_neighbors(app, ctx, area, scroll_layout, visible_rows);
    }
    ViewMode::Viewer => {
      let capacity = layout::grid_slots(area, &app.layout).len().max(1);
      let start = app.grid_start_page;
      let end = start.saturating_add(capacity).min(app.document.page_count);
      let visible = (start..end).collect::<Vec<_>>();
      preload_grid_neighbors(app, ctx, area, &visible);
    }
    ViewMode::Bookmarks => preload_bookmark_previews(app, ctx, area),
    ViewMode::Search if app.search_preload_ready() => preload_search_previews(app, ctx, area),
    ViewMode::Selection => preload_selection_history(app, ctx, area),
    ViewMode::Search | ViewMode::Metadata => {}
  }
}

pub(super) fn preload_scroll_neighbors(
  app: &App,
  ctx: &mut PreloadCtx<'_>,
  area: Rect,
  scroll_layout: &ScrollLayout,
  visible_rows: Range<usize>,
) {
  if visible_rows.is_empty() {
    return;
  }
  let limits = PreloadLimits::new(&app.settings.config.render);
  let row_count = scroll_layout.rows.len();
  let ahead_rows = visible_rows.end..visible_rows.end.saturating_add(limits.ahead).min(row_count);
  let behind_rows = visible_rows.start.saturating_sub(limits.behind)..visible_rows.start;
  // One slice job renders every slice of a page group, so queue each group
  // once even when several of its slices are in the window.
  let mut slice_groups = HashSet::new();

  preload_pages_in_rows(app, ctx, area, scroll_layout, ahead_rows.clone());
  preload_pages_after_rows(app, ctx, area, scroll_layout, visible_rows);
  for (distance, row_index) in ahead_rows.enumerate() {
    let layers = (
      distance < limits.slice_ahead,
      distance < limits.terminal_ahead,
    );
    preload_scroll_row(
      app,
      ctx,
      area,
      scroll_layout,
      row_index,
      layers,
      &mut slice_groups,
    );
  }

  preload_pages_in_rows(app, ctx, area, scroll_layout, behind_rows.clone().rev());
  for (distance, row_index) in behind_rows.rev().enumerate() {
    let layers = (
      distance < limits.slice_behind,
      distance < limits.terminal_behind,
    );
    preload_scroll_row(
      app,
      ctx,
      area,
      scroll_layout,
      row_index,
      layers,
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
  fn from_spec(spec: PageSliceSpec) -> Self {
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

/// Queues the full-page PNG of every page appearing in `rows`, in order.
fn preload_pages_in_rows(
  app: &App,
  ctx: &mut PreloadCtx<'_>,
  area: Rect,
  scroll_layout: &ScrollLayout,
  rows: impl Iterator<Item = usize>,
) {
  let mut seen = HashSet::new();
  for row_index in rows {
    for item in scroll_layout.row_items(row_index) {
      if seen.insert(item.page_index) {
        preload_scroll_page(app, ctx, area, *item);
      }
    }
  }
}

/// Queues the pages right after the last visible one, one raster batch
/// deep, so batched backends render them together.
fn preload_pages_after_rows(
  app: &App,
  ctx: &mut PreloadCtx<'_>,
  area: Rect,
  scroll_layout: &ScrollLayout,
  rows: Range<usize>,
) {
  let Some(last_visible_page) = rows
    .flat_map(|row_index| scroll_layout.row_items(row_index))
    .map(|item| item.page_index)
    .max()
  else {
    return;
  };
  let end = last_visible_page
    .saturating_add(app.document.pdf_raster_batch_pages.max(1))
    .min(app.document.page_count.saturating_sub(1));
  for page_index in last_visible_page.saturating_add(1)..=end {
    if let Some(item) = scroll_layout.first_item_of_page(page_index) {
      preload_scroll_page(app, ctx, area, *item);
    }
  }
}

fn preload_scroll_page(app: &App, ctx: &mut PreloadCtx<'_>, area: Rect, item: ScrollItem) {
  let spec = slice_spec_for_item(app, item, area);
  ctx.pages.preload(
    spec.page_index,
    spec.target_width,
    spec.target_height,
    ctx.tx,
  );
}

fn preload_scroll_row(
  app: &App,
  ctx: &mut PreloadCtx<'_>,
  area: Rect,
  scroll_layout: &ScrollLayout,
  row_index: usize,
  (preload_slice, preload_terminal): (bool, bool),
  slice_groups: &mut HashSet<SlicePreloadGroup>,
) {
  for item in scroll_layout.row_items(row_index) {
    let spec = slice_spec_for_item(app, *item, area);
    let slice = app.slice_image(&spec);
    if preload_slice
      && slice.is_none()
      && app.page_image(spec.page_index).is_some()
      && slice_groups.insert(SlicePreloadGroup::from_spec(spec))
    {
      ctx.pages.preload_slice(spec, ctx.tx);
    }
    if preload_terminal && let Some(slice) = slice {
      ctx
        .renderer
        .preload(slice, item.width, item.height, RenderKind::Fit, ctx.tx);
    }
  }
}

pub(super) fn preload_grid_neighbors(
  app: &App,
  ctx: &mut PreloadCtx<'_>,
  area: Rect,
  visible: &[usize],
) {
  let (Some(&first), Some(&last)) = (visible.first(), visible.last()) else {
    return;
  };
  let limits = PreloadLimits::new(&app.settings.config.render);
  let Some(slot) = layout::grid_slots(area, &app.layout).first().copied() else {
    return;
  };
  let page_area = slot_content_area(slot, &app.layout);
  if page_area.width == 0 || page_area.height == 0 {
    return;
  }
  let ahead = (last + 1..=last.saturating_add(limits.ahead))
    .take_while(|index| *index < app.document.page_count);
  for (distance, index) in ahead.enumerate() {
    preload_fitted_page(app, ctx, index, page_area, distance < limits.terminal_ahead);
  }
  for (distance, index) in (first.saturating_sub(limits.behind)..first)
    .rev()
    .enumerate()
  {
    preload_fitted_page(
      app,
      ctx,
      index,
      page_area,
      distance < limits.terminal_behind,
    );
  }
}

pub(super) fn preload_bookmark_previews(app: &App, ctx: &mut PreloadCtx<'_>, area: Rect) {
  let preview = preview_area(area, app.bookmarks.left_ratio, app.bookmarks.right_ratio);
  let Some(selected) = app.bookmarks.selected else {
    return;
  };
  let visible = app.bookmarks.visible_indices();
  let Some(selected_pos) = visible.iter().position(|index| *index == selected) else {
    return;
  };
  let limits = PreloadLimits::new(&app.settings.config.render);
  let mut seen_pages = HashSet::new();
  for (pos, preload_terminal) in limits.list_window(selected_pos, visible.len()) {
    let Some(bookmark) = visible
      .get(pos)
      .and_then(|index| app.bookmarks.entries.get(*index))
    else {
      continue;
    };
    let page_index = bookmark
      .page_index
      .min(app.document.page_count.saturating_sub(1));
    if seen_pages.insert(page_index) {
      preload_fitted_page(app, ctx, page_index, preview, preload_terminal);
    }
  }
}

pub(super) fn preload_search_previews(app: &App, ctx: &mut PreloadCtx<'_>, area: Rect) {
  let search = &app.search;
  if search.query().is_empty() || search.results.is_empty() {
    return;
  }
  let preview = preview_area(area, search.left_ratio, search.right_ratio);
  let Some(selected) = search.selected else {
    return;
  };
  let limits = PreloadLimits::new(&app.settings.config.render);
  for (index, preload_terminal) in limits.list_window(selected, search.results.len()) {
    if let Some(result) = search.results.get(index) {
      preload_search_preview(app, ctx, result, preview, preload_terminal);
    }
  }
}

pub(super) fn preload_selection_history(app: &mut App, ctx: &mut PreloadCtx<'_>, area: Rect) {
  let Some(selected) = app.selection.index else {
    return;
  };
  if app.selection.history.is_empty() || area.width == 0 || area.height == 0 {
    return;
  }
  let limits = PreloadLimits::new(&app.settings.config.render);
  let window = limits
    .list_window(selected, app.selection.history.len())
    .filter_map(|(index, terminal)| Some((*app.selection.history.get(index)?, terminal)))
    .collect::<Vec<_>>();
  for (selection, preload_terminal) in window {
    preload_selection_preview(app, ctx, selection, area, preload_terminal);
  }
}

/// Queues page `page_index` fitted into `area`, plus its terminal render
/// when `preload_terminal` is set and the page PNG exists.
fn preload_fitted_page(
  app: &App,
  ctx: &mut PreloadCtx<'_>,
  page_index: usize,
  area: Rect,
  preload_terminal: bool,
) {
  let Some(image_area) = preload_page_png(app, ctx, page_index, area) else {
    return;
  };
  if preload_terminal && let Some(page) = app.page_image(page_index) {
    ctx.preload_terminal(page, image_area);
  }
}

/// Queues the page PNG for `area` and returns where the page would be
/// drawn, or `None` when it does not fit.
fn preload_page_png(
  app: &App,
  ctx: &mut PreloadCtx<'_>,
  page_index: usize,
  area: Rect,
) -> Option<Rect> {
  let (image_area, (target_width, target_height)) = fitted_page_request(app, page_index, area);
  if image_area.width == 0 || image_area.height == 0 {
    return None;
  }
  ctx
    .pages
    .preload(page_index, target_width, target_height, ctx.tx);
  Some(image_area)
}

fn preload_search_preview(
  app: &App,
  ctx: &mut PreloadCtx<'_>,
  result: &search::PdfSearchMatch,
  area: Rect,
  preload_terminal: bool,
) {
  let Some(image_area) = preload_page_png(app, ctx, result.page_index, area) else {
    return;
  };
  if !preload_terminal {
    return;
  }
  let Some(page) = app.page_image(result.page_index) else {
    return;
  };
  let steps = [OverlayStep::SearchHighlight(result.clone())];
  if let OverlayState::Ready(highlighted) = ctx.overlays.request(page, &steps, true, ctx.tx) {
    ctx.preload_terminal(&highlighted, image_area);
  }
}

fn preload_selection_preview(
  app: &mut App,
  ctx: &mut PreloadCtx<'_>,
  selected: selection::PdfSelection,
  area: Rect,
  preload_terminal: bool,
) {
  let (target_width, target_height) = selection::selection_preview_page_target(
    selected,
    area.width,
    area.height,
    app.terminal_cell_pixels,
  );
  let key = app.request_selection_image(selected, target_width, target_height, true, ctx.tx);
  if !preload_terminal {
    return;
  }
  let Some(crop) = app.selection.images.get(&key) else {
    return;
  };
  let image_area = fitted_page_area(
    area,
    app.terminal_cell_pixels,
    Some((crop.width.max(1), crop.height.max(1))),
  );
  if image_area.width > 0 && image_area.height > 0 {
    ctx.preload_terminal(crop, image_area);
  }
}

/// Inner area of the preview panel of a two-panel view.
fn preview_area(area: Rect, left_ratio: u16, right_ratio: u16) -> Rect {
  let (_, preview) = split_panels(area, left_ratio, right_ratio);
  safe_inner(preview, 1, 1)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn limits(
    ahead: usize,
    behind: usize,
    terminal_ahead: usize,
    terminal_behind: usize,
  ) -> PreloadLimits {
    PreloadLimits {
      ahead,
      behind,
      slice_ahead: ahead,
      slice_behind: behind,
      terminal_ahead,
      terminal_behind,
    }
  }

  #[test]
  fn list_window_is_clamped_and_marks_terminal_neighbors() {
    let window = limits(3, 2, 1, 1).list_window(5, 7).collect::<Vec<_>>();
    assert_eq!(window, vec![(3, false), (4, true), (5, true), (6, true)]);
    assert_eq!(limits(3, 2, 1, 1).list_window(0, 0).count(), 0);
    assert_eq!(
      limits(0, 0, 0, 0).list_window(0, 1).collect::<Vec<_>>(),
      vec![(0, true)]
    );
  }
}
