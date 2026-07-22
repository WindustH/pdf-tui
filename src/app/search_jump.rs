use crate::{
  layout::{self, ScrollItem, ScrollLayout},
  search::PdfSearchMatch,
};

use super::App;

impl App {
  pub(super) fn jump_to_search_match(&mut self, result: &PdfSearchMatch) {
    let jumped = if self.layout.is_scroll() {
      self.jump_scroll_to_search_match(result)
    } else {
      self.jump_grid_to_search_match(result)
    };
    if !jumped {
      self.set_progress_target(search_match_center_progress(result));
    }
  }

  fn jump_scroll_to_search_match(&mut self, result: &PdfSearchMatch) -> bool {
    let Some(viewport) = self.viewport else {
      return false;
    };
    if viewport.width == 0 || viewport.height == 0 {
      return false;
    }
    let scroll_layout = layout::compute_scroll_layout(
      self.document.page_count,
      viewport.width,
      viewport.height,
      &self.layout,
      |index| self.page_dimensions(index),
      self.terminal_cell_pixels,
    );
    let Some(target) = search_target_scroll_item(&scroll_layout, result) else {
      return false;
    };
    let max_row = layout::max_scroll_row_for_viewport(
      &scroll_layout,
      viewport.height,
      self.layout.scroll_divisor,
    );
    let mut best_row = None;
    let mut best_distance = f64::INFINITY;
    for start_row in 0..=max_row {
      let Some(screen_y) = search_target_screen_y(
        &scroll_layout,
        start_row,
        viewport.height,
        self.layout.scroll_divisor,
        target,
      ) else {
        continue;
      };
      let distance = (screen_y - f64::from(viewport.height) / 2.0).abs();
      if distance < best_distance {
        best_row = Some(start_row);
        best_distance = distance;
      }
    }
    let Some(best_row) = best_row else {
      return false;
    };
    self.scroll = best_row as u32;
    self.last_scroll_layout = Some(scroll_layout);
    self.pending_progress = None;
    self.update_focus_from_scroll();
    true
  }

  fn jump_grid_to_search_match(&mut self, result: &PdfSearchMatch) -> bool {
    if self.document.page_count == 0 || result.page_index >= self.document.page_count {
      return false;
    }
    let capacity = self.layout.grid_capacity().max(1);
    let row_step = usize::from(self.layout.columns.max(1));
    let max_start = self.document.page_count.saturating_sub(capacity);
    let viewport = self.viewport;
    let slots = viewport.map(|area| layout::grid_slots(area, &self.layout));
    let mut best_start = None;
    let mut best_distance = f64::INFINITY;
    let mut start = 0_usize;
    loop {
      let candidate = start.min(max_start);
      if result.page_index >= candidate && result.page_index < candidate.saturating_add(capacity) {
        let slot_index = result.page_index.saturating_sub(candidate);
        let distance = if let (Some(area), Some(slots)) = (viewport, slots.as_ref()) {
          slots
            .get(slot_index)
            .map(|slot| {
              let dx = f64::from(slot.x) + f64::from(slot.width) / 2.0
                - (f64::from(area.x) + f64::from(area.width) / 2.0);
              let dy = f64::from(slot.y) + f64::from(slot.height) / 2.0
                - (f64::from(area.y) + f64::from(area.height) / 2.0);
              dx * dx + dy * dy
            })
            .unwrap_or(f64::INFINITY)
        } else {
          let row = slot_index / row_step.max(1);
          let col = slot_index % row_step.max(1);
          let center_row = (usize::from(self.layout.rows.max(1)).saturating_sub(1)) as f64 / 2.0;
          let center_col = (row_step.max(1).saturating_sub(1)) as f64 / 2.0;
          (row as f64 - center_row).powi(2) + (col as f64 - center_col).powi(2)
        };
        if distance < best_distance {
          best_start = Some(candidate);
          best_distance = distance;
        }
      }
      if candidate == max_start {
        break;
      }
      start = start.saturating_add(row_step.max(1));
    }
    let Some(best_start) = best_start else {
      return false;
    };
    self.grid_start_page = best_start;
    self.focused_page = result.page_index;
    self.last_scroll_layout = None;
    self.pending_progress = None;
    true
  }
}

#[derive(Debug, Clone, Copy)]
struct SearchScrollTarget {
  row_index: usize,
  item_height: u16,
  local_y: f64,
}

fn search_match_center_progress(result: &PdfSearchMatch) -> f64 {
  result.page_index as f64
    + ((result.rect.y_min + result.rect.y_max) / 2.0 / result.page_height.max(1.0)).clamp(0.0, 1.0)
}

fn search_target_scroll_item(
  scroll_layout: &ScrollLayout,
  result: &PdfSearchMatch,
) -> Option<SearchScrollTarget> {
  let fraction =
    ((result.rect.y_min + result.rect.y_max) / 2.0 / result.page_height.max(1.0)).clamp(0.0, 1.0);
  let mut best = None;
  let mut best_distance = f64::INFINITY;
  for item in scroll_layout
    .items
    .iter()
    .filter(|item| item.page_index == result.page_index)
  {
    let (top, bottom) = scroll_item_fraction_bounds(*item);
    let local_fraction = if bottom > top {
      ((fraction - top) / (bottom - top)).clamp(0.0, 1.0)
    } else {
      0.5
    };
    let local_y = local_fraction * f64::from(item.height.max(1));
    let distance = if fraction >= top && fraction <= bottom {
      0.0
    } else {
      (fraction - ((top + bottom) / 2.0)).abs()
    };
    if distance < best_distance {
      best = Some(SearchScrollTarget {
        row_index: item.row_index,
        item_height: item.height,
        local_y,
      });
      best_distance = distance;
    }
  }
  best
}

fn search_target_screen_y(
  scroll_layout: &ScrollLayout,
  start_row: usize,
  viewport_height: u16,
  scroll_divisor: u16,
  target: SearchScrollTarget,
) -> Option<f64> {
  let visible_rows =
    layout::visible_scroll_rows(scroll_layout, start_row, viewport_height, scroll_divisor);
  if !visible_rows.contains(&target.row_index) {
    return None;
  }
  let used_height = layout::visible_rows_height(scroll_layout, &visible_rows);
  let mut row_y = f64::from(viewport_height.saturating_sub(used_height)) / 2.0;
  for (position, row_index) in visible_rows.iter().copied().enumerate() {
    let row = scroll_layout.rows.get(row_index)?;
    if row_index == target.row_index {
      let item_y = f64::from(row.height.saturating_sub(target.item_height)) / 2.0;
      return Some(row_y + item_y + target.local_y);
    }
    row_y += f64::from(row.height);
    if position + 1 < visible_rows.len() {
      row_y += f64::from(row.gap_after);
    }
  }
  None
}

fn scroll_item_fraction_bounds(item: ScrollItem) -> (f64, f64) {
  let full_height = u32::from(item.full_height.max(1));
  let slice_count = u32::from(item.slice_count.max(1));
  let slice_index = u32::from(item.slice_index.min(item.slice_count.saturating_sub(1)));
  let top_cells = full_height.saturating_mul(slice_index) / slice_count;
  let bottom_cells = full_height.saturating_mul(slice_index.saturating_add(1)) / slice_count;
  (
    f64::from(top_cells) / f64::from(full_height.max(1)),
    f64::from(bottom_cells.max(top_cells.saturating_add(1))) / f64::from(full_height.max(1)),
  )
}
