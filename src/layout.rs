use ratatui::layout::Rect;

use crate::config::EffectiveLayoutConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollItem {
  pub page_index: usize,
  pub slice_index: u16,
  pub slice_count: u16,
  /// Height in cells of the tallest page in this row group; the shared
  /// slicing grid is derived from it so every page cuts at the same
  /// absolute cell boundaries.
  pub grid_height: u16,
  pub row_index: usize,
  pub x: u16,
  pub y: u32,
  pub width: u16,
  pub height: u16,
  pub full_width: u16,
  pub full_height: u16,
}

#[derive(Debug, Clone)]
pub struct ScrollRow {
  pub height: u16,
  pub gap_after: u16,
  pub items: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct ScrollLayout {
  pub items: Vec<ScrollItem>,
  pub rows: Vec<ScrollRow>,
  pub total_height: u32,
}

#[derive(Debug, Clone, Copy)]
struct ScrollWidthScore {
  average: f64,
  minimum: f64,
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
  let gap_x = config.gap_x;
  let max_page_width = fit_slot(viewport_width, columns, gap_x).max(1);
  let dimensions = (0..count).map(dimensions).collect::<Vec<_>>();
  let page_width = optimal_scroll_page_width(
    count,
    viewport_width,
    viewport_height,
    max_page_width,
    config,
    &dimensions,
    cell_pixels,
  );
  build_scroll_layout(
    count,
    viewport_width,
    viewport_height,
    page_width,
    config,
    &dimensions,
    cell_pixels,
  )
}

fn build_scroll_layout(
  count: usize,
  viewport_width: u16,
  viewport_height: u16,
  page_width: u16,
  config: &EffectiveLayoutConfig,
  dimensions: &[Option<(u32, u32)>],
  cell_pixels: Option<(u16, u16)>,
) -> ScrollLayout {
  let columns = config.columns.max(1) as usize;
  let gap_x = config.gap_x;
  let gap_y = config.gap_y;
  let x_offset = centered_offset(viewport_width, columns, page_width, gap_x);
  let mut items = Vec::new();
  let mut rows = Vec::new();
  let mut y = 0_u32;

  for row_start in (0..count).step_by(columns) {
    let page_end = (row_start + columns).min(count);
    let page_heights = (row_start..page_end)
      .map(|index| {
        page_height_cells(
          page_width,
          dimensions.get(index).copied().flatten(),
          cell_pixels,
        )
        .max(1)
      })
      .collect::<Vec<_>>();
    let slice_height_limit = slice_height_limit(viewport_height, config.scroll_divisor);
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
      let mut row_height = 1_u16;
      let mut row_items = Vec::new();
      for (offset, index) in (row_start..page_end).enumerate() {
        let full_height = page_heights[offset];
        let (_, height) = grid_slice_span(grid_height, max_slice_count, slice_row, full_height);
        if height == 0 {
          continue;
        }
        let height = height.max(1);
        row_height = row_height.max(height);
        let col = index - row_start;
        row_items.push(ScrollItem {
          page_index: index,
          slice_index: slice_row,
          slice_count: max_slice_count,
          grid_height,
          row_index: rows.len(),
          x: x_offset.saturating_add((col as u16).saturating_mul(page_width.saturating_add(gap_x))),
          y,
          width: page_width,
          height,
          full_width: page_width,
          full_height,
        });
      }

      let row_index = rows.len();
      let row_start = items.len();
      for mut item in row_items {
        item.row_index = row_index;
        items.push(item);
      }
      let row_end = items.len();
      let gap_after = if slice_row.saturating_add(1) < max_slice_count || page_end >= count {
        0
      } else {
        gap_y
      };
      rows.push(ScrollRow {
        height: row_height,
        gap_after,
        items: (row_start..row_end).collect(),
      });
      y = y
        .saturating_add(u32::from(row_height))
        .saturating_add(u32::from(gap_after));
    }
  }

  let total_height = y;
  ScrollLayout {
    items,
    rows,
    total_height,
  }
}

fn optimal_scroll_page_width(
  count: usize,
  viewport_width: u16,
  viewport_height: u16,
  max_page_width: u16,
  config: &EffectiveLayoutConfig,
  dimensions: &[Option<(u32, u32)>],
  cell_pixels: Option<(u16, u16)>,
) -> u16 {
  if count == 0 || viewport_width == 0 || viewport_height == 0 {
    return max_page_width.max(1);
  }

  let mut best_width = max_page_width.max(1);
  let mut best_score = ScrollWidthScore {
    average: f64::NEG_INFINITY,
    minimum: f64::NEG_INFINITY,
  };
  for page_width in 1..=max_page_width.max(1) {
    let layout = build_scroll_layout(
      count,
      viewport_width,
      viewport_height,
      page_width,
      config,
      dimensions,
      cell_pixels,
    );
    let score = scroll_width_score(
      &layout,
      viewport_width,
      viewport_height,
      config.scroll_divisor,
    );
    if better_scroll_width(score, page_width, best_score, best_width) {
      best_width = page_width;
      best_score = score;
    }
  }
  best_width
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

fn scroll_width_score(
  layout: &ScrollLayout,
  viewport_width: u16,
  viewport_height: u16,
  scroll_divisor: u16,
) -> ScrollWidthScore {
  if layout.rows.is_empty() || viewport_width == 0 || viewport_height == 0 {
    return ScrollWidthScore {
      average: 0.0,
      minimum: 0.0,
    };
  }
  let max_row = max_scroll_row_for_viewport(layout, viewport_height, scroll_divisor);
  let denominator = f64::from(viewport_width.max(1)) * f64::from(viewport_height.max(1));
  let mut total = 0.0;
  let mut minimum = f64::INFINITY;
  let mut count = 0_usize;
  for start_row in 0..=max_row {
    let rows = visible_scroll_rows(layout, start_row, viewport_height, scroll_divisor);
    let area = visible_content_area(layout, &rows);
    let score = area / denominator;
    total += score;
    minimum = minimum.min(score);
    count += 1;
  }
  ScrollWidthScore {
    average: if count == 0 {
      0.0
    } else {
      total / count as f64
    },
    minimum: if minimum.is_finite() { minimum } else { 0.0 },
  }
}

fn visible_content_area(layout: &ScrollLayout, rows: &[usize]) -> f64 {
  let mut area = 0.0;
  for row_index in rows {
    let Some(row) = layout.rows.get(*row_index) else {
      continue;
    };
    for item_index in &row.items {
      let Some(item) = layout.items.get(*item_index) else {
        continue;
      };
      area += f64::from(item.width.max(1)) * f64::from(item.height.max(1));
    }
  }
  area
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

pub fn grid_slice_span(
  grid_height: u16,
  slice_count: u16,
  slice_index: u16,
  full_height: u16,
) -> (u32, u16) {
  let slice_count = slice_count.max(1);
  let slice_index = slice_index.min(slice_count.saturating_sub(1));
  let grid = u32::from(grid_height.max(1));
  let full = u32::from(full_height.max(1));
  let start = (grid * u32::from(slice_index) / u32::from(slice_count)).min(full);
  let end = (grid * u32::from(slice_index.saturating_add(1)) / u32::from(slice_count)).min(full);
  (
    start,
    end.saturating_sub(start).min(u32::from(u16::MAX)) as u16,
  )
}

pub fn page_height_cells(
  width_cells: u16,
  dimensions: Option<(u32, u32)>,
  cell_pixels: Option<(u16, u16)>,
) -> u16 {
  let (page_width, page_height) = dimensions.unwrap_or((595, 842));
  let (cell_width, cell_height) = cell_pixels.unwrap_or((8, 16));
  let pixel_width = f64::from(width_cells.max(1)) * f64::from(cell_width.max(1));
  let pixel_height = pixel_width * f64::from(page_height.max(1)) / f64::from(page_width.max(1));
  (pixel_height / f64::from(cell_height.max(1)))
    .ceil()
    .clamp(1.0, f64::from(u16::MAX)) as u16
}

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
      slots.push(Rect {
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
      });
    }
  }
  slots
}

pub fn visible_scroll_rows(
  scroll_layout: &ScrollLayout,
  start_row: usize,
  viewport_height: u16,
  scroll_divisor: u16,
) -> Vec<usize> {
  if scroll_layout.rows.is_empty() || viewport_height == 0 {
    return Vec::new();
  }
  let max_rows = usize::from(scroll_divisor.max(1));
  let start_row = start_row.min(scroll_layout.rows.len().saturating_sub(1));
  let mut rows: Vec<usize> = Vec::new();
  let mut used = 0_u16;
  for row_index in start_row..scroll_layout.rows.len() {
    if rows.len() >= max_rows {
      break;
    }
    let Some(row) = scroll_layout.rows.get(row_index) else {
      break;
    };
    let extra_gap = rows
      .last()
      .and_then(|previous| scroll_layout.rows.get(*previous))
      .map(|row| row.gap_after)
      .unwrap_or(0);
    let candidate = used.saturating_add(extra_gap).saturating_add(row.height);
    if !rows.is_empty() && candidate > viewport_height {
      break;
    }
    rows.push(row_index);
    used = candidate;
  }
  if rows.is_empty() {
    rows.push(start_row);
  }
  rows
}

pub fn max_scroll_row_for_viewport(
  scroll_layout: &ScrollLayout,
  viewport_height: u16,
  scroll_divisor: u16,
) -> usize {
  if scroll_layout.rows.is_empty() || viewport_height == 0 {
    return 0;
  }
  let last_row = scroll_layout.rows.len().saturating_sub(1);
  for start_row in 0..=last_row {
    if visible_scroll_rows(scroll_layout, start_row, viewport_height, scroll_divisor)
      .last()
      .is_some_and(|row| *row == last_row)
    {
      return start_row;
    }
  }
  last_row
}

pub fn visible_rows_height(scroll_layout: &ScrollLayout, rows: &[usize]) -> u16 {
  let mut total = 0_u16;
  for (position, row_index) in rows.iter().copied().enumerate() {
    let Some(row) = scroll_layout.rows.get(row_index) else {
      continue;
    };
    total = total.saturating_add(row.height);
    if position + 1 < rows.len() {
      total = total.saturating_add(row.gap_after);
    }
  }
  total
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

#[cfg(test)]
mod tests {
  use super::*;

  fn scroll_config(columns: u16) -> EffectiveLayoutConfig {
    EffectiveLayoutConfig {
      name: "test".into(),
      strategy: "scroll".into(),
      columns,
      rows: 1,
      scroll_divisor: 1,
      gap_x: 0,
      gap_y: 0,
      show_border: false,
      padding: 0,
    }
  }

  #[test]
  fn grid_slice_span_clips_to_page_height() {
    // A 120-cell grid cut into 3 slices has boundaries 0/40/80/120.
    assert_eq!(grid_slice_span(120, 3, 0, 120), (0, 40));
    assert_eq!(grid_slice_span(120, 3, 2, 120), (80, 40));
    // A shorter page clips to its own height: slice 1 stops at 60, slice 2
    // is gone entirely.
    assert_eq!(grid_slice_span(120, 3, 1, 60), (40, 20));
    assert_eq!(grid_slice_span(120, 3, 2, 60), (60, 0));
    // Out-of-range indices clamp to the last slice.
    assert_eq!(
      grid_slice_span(120, 3, 9, 60),
      grid_slice_span(120, 3, 2, 60)
    );
  }

  #[test]
  fn mixed_height_pages_share_the_tallest_grid() {
    let config = scroll_config(2);
    let dims = [Some((100_u32, 300_u32)), Some((100_u32, 150_u32))];
    let layout = build_scroll_layout(2, 80, 50, 40, &config, &dims, Some((1, 1)));

    // The tallest page (120 cells, slice limit 50) dictates a 3-slice grid.
    let tall: Vec<_> = layout.items.iter().filter(|i| i.page_index == 0).collect();
    let short: Vec<_> = layout.items.iter().filter(|i| i.page_index == 1).collect();
    assert_eq!(tall.len(), 3);
    assert_eq!(short.len(), 2);
    for item in &layout.items {
      assert_eq!(item.slice_count, 3);
      assert_eq!(item.grid_height, 120);
    }
    assert_eq!(
      tall.iter().map(|i| i.height).collect::<Vec<_>>(),
      vec![40, 40, 40]
    );
    assert_eq!(
      short.iter().map(|i| i.height).collect::<Vec<_>>(),
      vec![40, 20]
    );

    // Every slice row hosts both columns while both have content; the
    // shorter page simply stops after its last slice.
    assert_eq!(layout.rows.len(), 3);
    assert_eq!(layout.rows[0].items.len(), 2);
    assert_eq!(layout.rows[1].items.len(), 2);
    assert_eq!(layout.rows[2].items.len(), 1);
    assert_eq!(layout.rows[0].height, 40);
    assert_eq!(layout.rows[1].height, 40);

    // Columns align at the top of each row and slices stay contiguous
    // within a page: no blank bands mid-page while scrolling.
    assert_eq!(short[0].y, tall[0].y);
    for pair in tall.windows(2) {
      assert_eq!(pair[1].y, pair[0].y + u32::from(pair[0].height));
    }
    assert_eq!(short[1].y, short[0].y + 40);
    assert_eq!(layout.total_height, 120);
  }

  #[test]
  fn uniform_pages_keep_proportional_slicing() {
    let config = scroll_config(2);
    let dims = [Some((100_u32, 300_u32)), Some((100_u32, 300_u32))];
    let layout = build_scroll_layout(2, 80, 50, 40, &config, &dims, Some((1, 1)));
    assert_eq!(layout.items.len(), 6);
    for item in &layout.items {
      assert_eq!(item.slice_count, 3);
      assert_eq!(item.grid_height, item.full_height);
      assert_eq!(item.height, 40);
    }
  }
}
