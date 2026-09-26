use crate::layout::ScrollLayout;

use super::App;

impl App {
  /// Position to persist on exit, or `None` for empty documents where
  /// there is nothing worth remembering.
  pub fn save_progress_on_exit(&self) -> Option<f64> {
    (self.document.page_count > 0)
      .then(|| self.current_progress().or(self.pending_progress))
      .flatten()
  }

  pub(super) fn current_progress(&self) -> Option<f64> {
    if self.document.page_count == 0 {
      return Some(0.0);
    }
    if self.layout.is_scroll() {
      let cached = self.scroll_layout.as_ref()?;
      return progress_for_scroll_row(
        cached.layout(),
        self.scroll as usize,
        cached.viewport_height(),
        cached.scroll_divisor(),
      );
    }
    progress_for_grid_start(
      self.grid_start_page,
      self.layout.grid_capacity(),
      self.document.page_count,
    )
  }

  /// Moves to a 0-based reading progress; applied once the layout for the
  /// current viewport exists if it does not yet.
  pub fn set_progress_target(&mut self, progress: f64) {
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
      let Some(cached) = &self.scroll_layout else {
        return false;
      };
      self.scroll = best_scroll_row_for_progress(
        cached.layout(),
        cached.viewport_height(),
        cached.scroll_divisor(),
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
  let max_row = layout.max_scroll_row(viewport_height, scroll_divisor);
  closest_candidate(0..=max_row, target, |row_index| {
    progress_for_scroll_row(layout, row_index, viewport_height, scroll_divisor)
  })
  .unwrap_or(0)
}

/// The candidate whose progress is closest to `target`. Ties go to the
/// later candidate, so an integer target such as a page start (`4.0`, the
/// boundary between the fourth and fifth page) lands on the page that
/// begins there rather than on the one that ends there.
fn closest_candidate(
  candidates: impl IntoIterator<Item = usize>,
  target: f64,
  progress_of: impl Fn(usize) -> Option<f64>,
) -> Option<usize> {
  const EPSILON: f64 = 1e-9;
  let mut best = None;
  let mut best_distance = f64::INFINITY;
  for candidate in candidates {
    let Some(progress) = progress_of(candidate) else {
      continue;
    };
    let distance = (progress - target).abs();
    if distance <= best_distance + EPSILON {
      best = Some(candidate);
      best_distance = best_distance.min(distance);
    }
  }
  best
}

fn progress_for_scroll_row(
  layout: &ScrollLayout,
  row_index: usize,
  viewport_height: u16,
  scroll_divisor: u16,
) -> Option<f64> {
  let mut weighted_sum = 0.0;
  let mut total_weight = 0.0;
  for row_index in layout.visible_rows(row_index, viewport_height, scroll_divisor) {
    for item in layout.row_items(row_index) {
      let full_width = f64::from(item.full_width.max(1));
      let full_height = f64::from(item.full_height.max(1));
      let (top_cells, height_cells) = crate::layout::grid_slice_span(
        item.grid_height,
        item.slice_count,
        item.slice_index,
        item.full_height,
      );
      let top = f64::from(top_cells) / full_height;
      let bottom = f64::from(top_cells.saturating_add(u32::from(height_cells))) / full_height;
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
  closest_candidate(
    reachable_grid_starts(page_count, capacity, row_step),
    target,
    |start| progress_for_grid_start(start, capacity, page_count),
  )
  .unwrap_or(0)
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
  use crate::layout::{ScrollItem, ScrollRow};

  #[test]
  fn grid_progress_uses_only_reachable_row_starts() {
    assert_eq!(reachable_grid_starts(20, 6, 3), vec![0, 3, 6, 9, 12, 14]);
    assert_eq!(best_grid_start_for_progress(0.0, 6, 3, 20), 0);
    assert_eq!(best_grid_start_for_progress(6.0, 6, 3, 20), 3);
    assert_eq!(best_grid_start_for_progress(18.0, 6, 3, 20), 14);
  }

  #[test]
  fn page_start_targets_land_on_the_page_that_begins_there() {
    // Three one-row pages, one row per screen: rows report progress
    // 0.5 / 1.5 / 2.5, so 1.0 is equally far from rows 0 and 1.
    let layout = ScrollLayout {
      items: (0..3)
        .map(|page_index| ScrollItem {
          page_index,
          slice_index: 0,
          slice_count: 1,
          grid_height: 10,
          row_index: page_index,
          x: 0,
          y: page_index as u32 * 10,
          width: 10,
          height: 10,
          full_width: 10,
          full_height: 10,
        })
        .collect(),
      rows: (0..3)
        .map(|index| ScrollRow {
          height: 10,
          gap_after: 0,
          items: vec![index],
        })
        .collect(),
      total_height: 30,
    };
    assert_eq!(best_scroll_row_for_progress(&layout, 10, 1, 1.0), 1);
    assert_eq!(best_scroll_row_for_progress(&layout, 10, 1, 2.0), 2);
    assert_eq!(best_grid_start_for_progress(1.0, 1, 1, 3), 1);
  }

  #[test]
  fn progress_is_zero_based() {
    assert_eq!(progress_for_grid_start(0, 1, 10), Some(0.5));
    assert_eq!(best_grid_start_for_progress(0.0, 1, 1, 10), 0);
  }

  #[test]
  fn scroll_progress_uses_shared_grid_slice_bounds() {
    // A 60-cell page inside a row group whose tallest page dictates a
    // 120-cell, 3-slice grid (boundaries 0/40/80/120). Its middle slice
    // covers cells 40..60, i.e. fractions 2/3..1 of the page, not the
    // 1/3..2/3 that per-page proportional slicing would report.
    let item = ScrollItem {
      page_index: 1,
      slice_index: 1,
      slice_count: 3,
      grid_height: 120,
      row_index: 0,
      x: 0,
      y: 40,
      width: 40,
      height: 20,
      full_width: 40,
      full_height: 60,
    };
    let layout = ScrollLayout {
      items: vec![item],
      rows: vec![ScrollRow {
        height: 20,
        gap_after: 0,
        items: vec![0],
      }],
      total_height: 60,
    };
    let progress = progress_for_scroll_row(&layout, 0, 50, 1).unwrap();
    assert!(
      (progress - 11.0 / 6.0).abs() < 1e-9,
      "progress = {progress}"
    );
    // The jump chain inverts the same math: that row is the best landing
    // spot for the progress it reports.
    assert_eq!(best_scroll_row_for_progress(&layout, 50, 1, 11.0 / 6.0), 0);
  }
}
