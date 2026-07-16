use ratatui::layout::Rect;

use crate::config::EffectiveLayoutConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollItem {
  pub page_index: usize,
  pub slice_index: u16,
  pub slice_count: u16,
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
  let gap_y = config.gap_y;
  let page_width = fit_slot(viewport_width, columns, gap_x).max(1);
  let x_offset = centered_offset(viewport_width, columns, page_width, gap_x);
  let mut items = Vec::new();
  let mut rows = Vec::new();
  let mut y = 0_u32;

  for row_start in (0..count).step_by(columns) {
    let page_end = (row_start + columns).min(count);
    let page_heights = (row_start..page_end)
      .map(|index| page_height_cells(page_width, dimensions(index), cell_pixels).max(1))
      .collect::<Vec<_>>();
    let slice_height_limit = slice_height_limit(viewport_height, config.scroll_divisor);
    let slice_counts = page_heights
      .iter()
      .map(|height| slice_count(*height, slice_height_limit))
      .collect::<Vec<_>>();
    let max_slice_count = slice_counts.iter().copied().max().unwrap_or(1);

    for slice_row in 0..max_slice_count {
      let mut row_height = 1_u16;
      let mut row_items = Vec::new();
      for (offset, index) in (row_start..page_end).enumerate() {
        let slice_count = slice_counts[offset];
        if slice_row >= slice_count {
          continue;
        }
        let full_height = page_heights[offset];
        let height = slice_height_for(full_height, slice_count, slice_row).max(1);
        row_height = row_height.max(height);
        let col = index - row_start;
        row_items.push(ScrollItem {
          page_index: index,
          slice_index: slice_row,
          slice_count,
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
        item.y = item
          .y
          .saturating_add(u32::from(row_height.saturating_sub(item.height)) / 2);
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

pub fn slice_height_limit(viewport_height: u16, scroll_divisor: u16) -> u16 {
  let divisor = scroll_divisor.max(1);
  viewport_height
    .max(1)
    .saturating_add(divisor.saturating_sub(1))
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

pub fn slice_height_for(full_height: u16, slice_count: u16, slice_index: u16) -> u16 {
  let slice_count = slice_count.max(1);
  let slice_index = slice_index.min(slice_count.saturating_sub(1));
  let start = (u32::from(full_height) * u32::from(slice_index)) / u32::from(slice_count);
  let end =
    (u32::from(full_height) * u32::from(slice_index.saturating_add(1))) / u32::from(slice_count);
  end.saturating_sub(start).max(1).min(u32::from(u16::MAX)) as u16
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
