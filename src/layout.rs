//! Page geometry for the two reading layouts: the slice-based scroll layout
//! and the fixed page grid.

use std::ops::Range;

use ratatui::layout::Rect;

use crate::{config::EffectiveLayoutConfig, geometry::DEFAULT_CELL_PIXELS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollItem {
  pub page_index: usize,
  pub slice_index: u16,
  pub slice_count: u16,
  /// Height in cells of the tallest page in this row group; the shared
  /// slicing grid is derived from it so every page cuts at the same
  /// absolute cell boundaries.
  pub grid_height: u16,
  pub scroll_divisor: u16,
  pub label_height: u16,
  pub row_index: usize,
  pub x: u16,
  pub y: u32,
  pub width: u16,
  pub height: u16,
  pub full_width: u16,
  pub full_height: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrollRow {
  pub height: u16,
  pub gap_after: u16,
  pub items: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrollLayout {
  pub items: Vec<ScrollItem>,
  pub rows: Vec<ScrollRow>,
  pub total_height: u32,
}

/// A scroll item together with the screen area it occupies for one scroll
/// position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacedScrollItem {
  pub item: ScrollItem,
  pub area: Rect,
  pub label_area: Rect,
}

#[derive(Debug, Clone, Copy)]
struct ScrollWidthScore {
  average: f64,
  minimum: f64,
}

impl ScrollLayout {
  /// Rows shown when `start_row` is the top row: consecutive rows that fit
  /// the viewport, and always at least
  /// one. Empty only for an empty layout or a zero-height viewport.
  pub fn visible_rows(
    &self,
    start_row: usize,
    viewport_height: u16,
    _scroll_divisor: u16,
  ) -> Range<usize> {
    if self.rows.is_empty() || viewport_height == 0 {
      return 0..0;
    }
    let start = start_row.min(self.rows.len() - 1);
    let mut end = start + 1;
    let mut used = u32::from(self.rows[start].height);
    while end < self.rows.len() {
      let candidate =
        used + u32::from(self.rows[end - 1].gap_after) + u32::from(self.rows[end].height);
      if candidate > u32::from(viewport_height) {
        break;
      }
      used = candidate;
      end += 1;
    }
    start..end
  }

  /// Largest useful scroll row: the first start row whose visible range
  /// reaches the last row. The last visible row never decreases as the
  /// start row grows, so scanning back from the end stops after at most one
  /// viewport of rows.
  pub fn max_scroll_row(&self, viewport_height: u16, scroll_divisor: u16) -> usize {
    if self.rows.is_empty() || viewport_height == 0 {
      return 0;
    }
    let last_row = self.rows.len();
    let mut start = last_row - 1;
    while start > 0
      && self
        .visible_rows(start - 1, viewport_height, scroll_divisor)
        .end
        == last_row
    {
      start -= 1;
    }
    start
  }

  /// Height of `rows` including the gaps between them.
  pub fn rows_height(&self, rows: Range<usize>) -> u16 {
    let mut total = 0_u16;
    for row_index in rows.clone() {
      let Some(row) = self.rows.get(row_index) else {
        continue;
      };
      total = total.saturating_add(row.height);
      if row_index + 1 < rows.end {
        total = total.saturating_add(row.gap_after);
      }
    }
    total
  }

  /// Screen placement of every visible item for `start_row`. The visible
  /// rows are centered vertically and slices are top-aligned within their
  /// row: under the shared slicing grid a shorter slice is always a page's
  /// remainder, so centering it would open a seam above it.
  pub fn placed_items(
    &self,
    start_row: usize,
    viewport: Rect,
    scroll_divisor: u16,
  ) -> Vec<PlacedScrollItem> {
    let rows = self.visible_rows(start_row, viewport.height, scroll_divisor);
    let used_height = self.rows_height(rows.clone());
    let mut row_y = viewport
      .y
      .saturating_add(viewport.height.saturating_sub(used_height) / 2);
    let mut placed = Vec::new();
    for row_index in rows.clone() {
      let row = &self.rows[row_index];
      for item in row.items.iter().filter_map(|index| self.items.get(*index)) {
        placed.push(PlacedScrollItem {
          item: *item,
          label_area: Rect::new(
            viewport.x.saturating_add(item.x),
            row_y,
            item.width.min(viewport.width.saturating_sub(item.x)),
            item.label_height,
          ),
          area: Rect::new(
            viewport.x.saturating_add(item.x),
            row_y.saturating_add(item.label_height),
            item.width.min(viewport.width.saturating_sub(item.x)),
            item.height,
          ),
        });
      }
      row_y = row_y.saturating_add(row.height);
      if row_index + 1 < rows.end {
        row_y = row_y.saturating_add(row.gap_after);
      }
    }
    placed
  }

  pub fn row_items(&self, row_index: usize) -> impl Iterator<Item = &ScrollItem> {
    self
      .rows
      .get(row_index)
      .into_iter()
      .flat_map(|row| row.items.iter())
      .filter_map(|index| self.items.get(*index))
  }

  /// The topmost item (first slice) of `page_index`.
  pub fn first_item_of_page(&self, page_index: usize) -> Option<&ScrollItem> {
    self.items.iter().find(|item| item.page_index == page_index)
  }

  /// Index of the first row showing any part of `page_index`.
  pub fn first_row_of_page(&self, page_index: usize) -> Option<usize> {
    self
      .first_item_of_page(page_index)
      .map(|item| item.row_index)
  }
}

pub fn compute_scroll_layout(
  count: usize,
  viewport_width: u16,
  viewport_height: u16,
  config: &EffectiveLayoutConfig,
  dimensions: impl Fn(usize) -> Option<(u32, u32)>,
  cell_pixels: Option<(u16, u16)>,
) -> ScrollLayout {
  let columns = config.columns.max(1) as usize;
  let max_page_width = fit_slot(viewport_width, columns, config.gap_x).max(1);
  let dimensions = (0..count).map(dimensions).collect::<Vec<_>>();
  let builder = ScrollLayoutBuilder {
    viewport_width,
    viewport_height,
    config,
    dimensions: &dimensions,
    cell_pixels,
  };
  let page_width = builder.optimal_page_width(max_page_width);
  builder.build(page_width)
}

struct ScrollLayoutBuilder<'a> {
  viewport_width: u16,
  viewport_height: u16,
  config: &'a EffectiveLayoutConfig,
  dimensions: &'a [Option<(u32, u32)>],
  cell_pixels: Option<(u16, u16)>,
}

impl ScrollLayoutBuilder<'_> {
  fn build(&self, page_width: u16) -> ScrollLayout {
    let count = self.dimensions.len();
    let columns = self.config.columns.max(1) as usize;
    let gap_x = self.config.gap_x;
    let gap_y = self.config.gap_y;
    let x_offset = centered_offset(self.viewport_width, columns, page_width, gap_x);
    let label_height = u16::from(self.config.show_page_numbers && self.viewport_height > 1);
    let slice_height_limit = slice_height_limit(
      self.viewport_height.saturating_sub(label_height),
      self.config.scroll_divisor,
    );
    let mut items = Vec::new();
    let mut rows = Vec::new();
    let mut y = 0_u32;

    for row_start in (0..count).step_by(columns) {
      let page_end = (row_start + columns).min(count);
      let page_heights = (row_start..page_end)
        .map(|index| {
          page_height_cells(
            page_width,
            self.dimensions.get(index).copied().flatten(),
            self.cell_pixels,
          )
          .max(1)
        })
        .collect::<Vec<_>>();
      // Pages with different aspect ratios must not slice independently:
      // mixed slice sizes get vertically centered per row, which punches
      // ragged blank bands into the shorter-sliced page while scrolling.
      // Instead the row group shares the tallest page's slicing grid and
      // every page cuts at the same absolute cell boundaries (clipped to
      // its own height), so slices stay contiguous and aligned across
      // columns; a shorter page simply runs out of content at its end.
      let grid_height = page_heights.iter().copied().max().unwrap_or(1);
      let max_slice_count = slice_count(grid_height, slice_height_limit);

      for slice_row in 0..max_slice_count {
        let row_index = rows.len();
        let row_first_item = items.len();
        let mut row_height = 1_u16;
        for (col, index) in (row_start..page_end).enumerate() {
          let full_height = page_heights[col];
          let (_, height) = grid_slice_span(grid_height, max_slice_count, slice_row, full_height);
          if height == 0 {
            continue;
          }
          let label_height = if slice_row == 0 { label_height } else { 0 };
          row_height = row_height.max(height.saturating_add(label_height));
          items.push(ScrollItem {
            page_index: index,
            slice_index: slice_row,
            slice_count: max_slice_count,
            grid_height,
            scroll_divisor: self.config.scroll_divisor,
            label_height,
            row_index,
            x: x_offset
              .saturating_add((col as u16).saturating_mul(page_width.saturating_add(gap_x))),
            y,
            width: page_width,
            height,
            full_width: page_width,
            full_height,
          });
        }
        let gap_after = if slice_row.saturating_add(1) < max_slice_count || page_end >= count {
          0
        } else {
          gap_y
        };
        rows.push(ScrollRow {
          height: row_height,
          gap_after,
          items: (row_first_item..items.len()).collect(),
        });
        y = y
          .saturating_add(u32::from(row_height))
          .saturating_add(u32::from(gap_after));
      }
    }

    ScrollLayout {
      items,
      rows,
      total_height: y,
    }
  }

  /// Page width that maximizes the average share of the viewport covered by
  /// page content over all scroll positions, preferring a better worst case
  /// and then a wider page on ties.
  fn optimal_page_width(&self, max_page_width: u16) -> u16 {
    let max_page_width = max_page_width.max(1);
    if self.dimensions.is_empty() || self.viewport_width == 0 || self.viewport_height == 0 {
      return max_page_width;
    }
    let mut best_width = max_page_width;
    let mut best_score = ScrollWidthScore {
      average: f64::NEG_INFINITY,
      minimum: f64::NEG_INFINITY,
    };
    // Large documents use a bounded set of width candidates; building all
    // rows for every terminal column would stall resize on thousands of pages.
    let stride = if self.dimensions.len() > 256 {
      usize::from(max_page_width).div_ceil(32).max(1)
    } else {
      1
    };
    for page_width in (1..=max_page_width)
      .step_by(stride)
      .chain(std::iter::once(max_page_width))
    {
      let score = self.width_score(&self.build(page_width));
      if better_scroll_width(score, page_width, best_score, best_width) {
        best_width = page_width;
        best_score = score;
      }
    }
    best_width
  }

  fn width_score(&self, layout: &ScrollLayout) -> ScrollWidthScore {
    if layout.rows.is_empty() {
      return ScrollWidthScore {
        average: 0.0,
        minimum: 0.0,
      };
    }
    // Content area per row as prefix sums, so each scroll position costs
    // O(1) once its visible range is known.
    let mut area_prefix = Vec::with_capacity(layout.rows.len() + 1);
    area_prefix.push(0_u64);
    for row_index in 0..layout.rows.len() {
      let area = layout
        .row_items(row_index)
        .map(|item| u64::from(item.width.max(1)) * u64::from(item.height.max(1)))
        .sum::<u64>();
      area_prefix.push(area_prefix[row_index] + area);
    }
    let divisor = self.config.scroll_divisor;
    let max_row = layout.max_scroll_row(self.viewport_height, divisor);
    let denominator = f64::from(self.viewport_width) * f64::from(self.viewport_height);
    let mut total = 0.0;
    let mut minimum = f64::INFINITY;
    for start_row in 0..=max_row {
      let rows = layout.visible_rows(start_row, self.viewport_height, divisor);
      let score = (area_prefix[rows.end] - area_prefix[rows.start]) as f64 / denominator;
      total += score;
      minimum = minimum.min(score);
    }
    ScrollWidthScore {
      average: total / (max_row + 1) as f64,
      minimum: if minimum.is_finite() { minimum } else { 0.0 },
    }
  }
}

fn better_scroll_width(
  score: ScrollWidthScore,
  width: u16,
  best_score: ScrollWidthScore,
  best_width: u16,
) -> bool {
  const EPSILON: f64 = 0.000_001;
  score.average > best_score.average + EPSILON
    || ((score.average - best_score.average).abs() <= EPSILON
      && (score.minimum > best_score.minimum + EPSILON
        || ((score.minimum - best_score.minimum).abs() <= EPSILON && width > best_width)))
}

pub fn slice_height_limit(viewport_height: u16, scroll_divisor: u16) -> u16 {
  let divisor = scroll_divisor.max(1);
  viewport_height
    .max(1)
    .checked_div(divisor)
    .unwrap_or(1)
    .max(1)
}

fn slice_count(full_height: u16, limit: u16) -> u16 {
  full_height
    .max(1)
    .saturating_add(limit.max(1).saturating_sub(1))
    .checked_div(limit.max(1))
    .unwrap_or(1)
    .max(1)
}

/// Cell offset and height of slice `slice_index` for a page of
/// `full_height` cells cut on the shared grid of `grid_height` cells.
pub fn grid_slice_span(
  grid_height: u16,
  slice_count: u16,
  slice_index: u16,
  full_height: u16,
) -> (u32, u16) {
  img_tui::slicing::SliceGrid::Even {
    height: grid_height.max(1),
    count: slice_count.max(1),
  }
  .span(slice_index.min(slice_count.max(1) - 1), full_height.max(1))
}

pub fn page_height_cells(
  width_cells: u16,
  dimensions: Option<(u32, u32)>,
  cell_pixels: Option<(u16, u16)>,
) -> u16 {
  let (page_width, page_height) = dimensions.unwrap_or((595, 842));
  let (cell_width, cell_height) = cell_pixels.unwrap_or(DEFAULT_CELL_PIXELS);
  let pixel_width = f64::from(width_cells.max(1)) * f64::from(cell_width.max(1));
  let pixel_height = pixel_width * f64::from(page_height.max(1)) / f64::from(page_width.max(1));
  (pixel_height / f64::from(cell_height.max(1)))
    .ceil()
    .clamp(1.0, f64::from(u16::MAX)) as u16
}

/// Slot rectangles of a grid layout, row by row.
pub fn grid_slots(area: Rect, config: &EffectiveLayoutConfig) -> Vec<Rect> {
  let rows = config.rows.max(1) as usize;
  let columns = config.columns.max(1) as usize;
  let gap_x = config.gap_x;
  let gap_y = config.gap_y;
  let cell_width = fit_slot(area.width, columns, gap_x).max(1);
  let cell_height = fit_slot(area.height, rows, gap_y).max(1);
  let x_offset = centered_offset(area.width, columns, cell_width, gap_x);
  let y_offset = centered_offset(area.height, rows, cell_height, gap_y);
  let mut slots = Vec::with_capacity(rows * columns);
  for row in 0..rows {
    for col in 0..columns {
      slots.push(
        Rect {
          x: area
            .x
            .saturating_add(x_offset)
            .saturating_add((col as u16).saturating_mul(cell_width.saturating_add(gap_x))),
          y: area
            .y
            .saturating_add(y_offset)
            .saturating_add((row as u16).saturating_mul(cell_height.saturating_add(gap_y))),
          width: cell_width,
          height: cell_height,
        }
        .intersection(area),
      );
    }
  }
  slots
}

fn fit_slot(total: u16, count: usize, gap: u16) -> u16 {
  if count == 0 {
    return total.max(1);
  }
  let gaps = gap.saturating_mul(count.saturating_sub(1) as u16);
  total
    .saturating_sub(gaps)
    .checked_div(count as u16)
    .unwrap_or(1)
}

fn centered_offset(total: u16, count: usize, item: u16, gap: u16) -> u16 {
  if count == 0 {
    return 0;
  }
  let used = (count as u16)
    .saturating_mul(item)
    .saturating_add(gap.saturating_mul(count.saturating_sub(1) as u16));
  total.saturating_sub(used) / 2
}
