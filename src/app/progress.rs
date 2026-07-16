use crate::layout::ScrollLayout;

use super::App;

impl App {
  pub fn set_user_progress_target(&mut self, progress: f64) {
    self.set_progress_target(progress);
  }

  pub(super) fn current_progress(&self) -> Option<f64> {
    if self.document.page_count == 0 {
      return Some(0.0);
    }
    if self.layout.is_scroll() {
      let layout = self.last_scroll_layout.as_ref()?;
      return progress_for_scroll_row(
        layout,
        self.scroll as usize,
        self.viewport_height,
        self.layout.scroll_divisor,
      );
    }
    progress_for_grid_start(
      self.grid_start_page,
      self.layout.grid_capacity(),
      self.document.page_count,
    )
  }

  pub(super) fn set_progress_target(&mut self, progress: f64) {
    let progress = self.clamp_progress(progress);
    if self.apply_progress_to_current_layout(progress) {
      self.pending_progress = None;
    } else {
      self.pending_progress = Some(progress);
    }
  }

  pub(super) fn apply_pending_progress_if_ready(&mut self) {
    let Some(progress) = self.pending_progress else {
      return;
    };
    if self.apply_progress_to_current_layout(progress) {
      self.pending_progress = None;
    }
  }

  fn apply_progress_to_current_layout(&mut self, progress: f64) -> bool {
    if self.document.page_count == 0 {
      self.scroll = 0;
      self.grid_start_page = 0;
      self.focused_page = 0;
      return true;
    }
    if self.layout.is_scroll() {
      let Some(layout) = &self.last_scroll_layout else {
        return false;
      };
      self.scroll = best_scroll_row_for_progress(
        layout,
        self.viewport_height,
        self.layout.scroll_divisor,
        progress,
      ) as u32;
      self.update_focus_from_scroll();
      true
    } else {
      let capacity = self.layout.grid_capacity().max(1);
      self.grid_start_page = best_grid_start_for_progress(
        progress,
        capacity,
        self.layout.columns.max(1) as usize,
        self.document.page_count,
      );
      self.focused_page = self.grid_start_page.min(self.document.page_count - 1);
      true
    }
  }

  pub(super) fn normalize_current_layout_state(&mut self) {
    if self.document.page_count == 0 {
      self.scroll = 0;
      self.grid_start_page = 0;
      self.focused_page = 0;
      return;
    }
    if self.layout.is_scroll() {
      self.grid_start_page = 0;
      self.scroll = self.scroll.min(self.max_scroll());
      self.update_focus_from_scroll();
    } else {
      let capacity = self.layout.grid_capacity();
      self.clamp_grid_start(capacity);
      self.focused_page = self.grid_start_page.min(self.document.page_count - 1);
    }
  }

  fn clamp_progress(&self, progress: f64) -> f64 {
    if !progress.is_finite() {
      return 0.0;
    }
    progress.clamp(0.0, self.document.page_count as f64)
  }
}

fn best_scroll_row_for_progress(
  layout: &ScrollLayout,
  viewport_height: u16,
  scroll_divisor: u16,
  target: f64,
) -> usize {
  if layout.rows.is_empty() {
    return 0;
  }
  let mut best_row = 0;
  let mut best_distance = f64::INFINITY;
  let max_row = crate::layout::max_scroll_row_for_viewport(layout, viewport_height, scroll_divisor);
  for row_index in 0..=max_row {
    let Some(progress) =
      progress_for_scroll_row(layout, row_index, viewport_height, scroll_divisor)
    else {
      continue;
    };
    let distance = (progress - target).abs();
    if distance < best_distance {
      best_row = row_index;
      best_distance = distance;
    }
  }
  best_row
}

fn progress_for_scroll_row(
  layout: &ScrollLayout,
  row_index: usize,
  viewport_height: u16,
  scroll_divisor: u16,
) -> Option<f64> {
  let visible_rows =
    crate::layout::visible_scroll_rows(layout, row_index, viewport_height, scroll_divisor);
  let mut weighted_sum = 0.0;
  let mut total_weight = 0.0;
  for row_index in visible_rows {
    let Some(row) = layout.rows.get(row_index) else {
      continue;
    };
    for item_index in &row.items {
      let Some(item) = layout.items.get(*item_index) else {
        continue;
      };
      let full_width = f64::from(item.full_width.max(1));
      let full_height = f64::from(item.full_height.max(1));
      let slice_count = item.slice_count.max(1);
      let top_cells =
        (u32::from(item.full_height) * u32::from(item.slice_index)) / u32::from(slice_count);
      let bottom_cells = (u32::from(item.full_height)
        * u32::from(item.slice_index.saturating_add(1)))
        / u32::from(slice_count);
      let top = f64::from(top_cells) / full_height;
      let bottom = f64::from(bottom_cells) / full_height;
      let width_fraction = f64::from(item.width.max(1)) / full_width;
      let height_fraction = (bottom - top).max(0.0);
      let weight = width_fraction * height_fraction;
      if weight <= 0.0 {
        continue;
      }
      let progress = item.page_index as f64 + (top + bottom) / 2.0;
      weighted_sum += progress * weight;
      total_weight += weight;
    }
  }
  (total_weight > 0.0).then_some(weighted_sum / total_weight)
}

fn best_grid_start_for_progress(
  target: f64,
  capacity: usize,
  row_step: usize,
  page_count: usize,
) -> usize {
  if page_count == 0 {
    return 0;
  }
  let mut best_start = 0;
  let mut best_distance = f64::INFINITY;
  for start in reachable_grid_starts(page_count, capacity, row_step) {
    let Some(progress) = progress_for_grid_start(start, capacity, page_count) else {
      continue;
    };
    let distance = (progress - target).abs();
    if distance < best_distance {
      best_start = start;
      best_distance = distance;
    }
  }
  best_start
}

fn reachable_grid_starts(page_count: usize, capacity: usize, row_step: usize) -> Vec<usize> {
  if page_count == 0 {
    return vec![0];
  }
  let capacity = capacity.max(1);
  let row_step = row_step.max(1);
  let max_start = page_count.saturating_sub(capacity);
  let mut starts = Vec::new();
  let mut start = 0;
  loop {
    let clamped = start.min(max_start);
    if starts.last().copied() != Some(clamped) {
      starts.push(clamped);
    }
    if clamped == max_start {
      break;
    }
    start = start.saturating_add(row_step);
  }
  starts
}

fn progress_for_grid_start(start: usize, capacity: usize, page_count: usize) -> Option<f64> {
  if page_count == 0 {
    return Some(0.0);
  }
  let start = start.min(page_count.saturating_sub(1));
  let visible_count = page_count.saturating_sub(start).min(capacity.max(1));
  if visible_count == 0 {
    return None;
  }
  let first = start as f64 + 0.5;
  let last = start.saturating_add(visible_count - 1) as f64 + 0.5;
  Some((first + last) / 2.0)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn grid_progress_uses_only_reachable_row_starts() {
    assert_eq!(reachable_grid_starts(20, 6, 3), vec![0, 3, 6, 9, 12, 14]);
    assert_eq!(best_grid_start_for_progress(0.0, 6, 3, 20), 0);
    assert_eq!(best_grid_start_for_progress(6.0, 6, 3, 20), 3);
    assert_eq!(best_grid_start_for_progress(18.0, 6, 3, 20), 14);
  }

  #[test]
  fn progress_is_zero_based() {
    assert_eq!(progress_for_grid_start(0, 1, 10), Some(0.5));
    assert_eq!(best_grid_start_for_progress(0.0, 1, 1, 10), 0);
  }
}
