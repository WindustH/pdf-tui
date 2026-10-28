//! Independent geometry and progress for the follower of a composed layout.

use super::{
  App,
  progress::{best_grid_start_for_progress, best_scroll_row_for_progress},
};
use crate::{
  config::EffectiveLayoutConfig,
  layout::{self, ScrollLayout},
};
use ratatui::layout::Rect;

pub(crate) struct FollowerPanel {
  pub config: EffectiveLayoutConfig,
  pub area: Rect,
  pub scroll_layout: Option<ScrollLayout>,
  pub start: usize,
  cell_pixels: Option<(u16, u16)>,
  progress: Option<f64>,
}

impl App {
  pub(crate) fn prepare_follower(&mut self, config: EffectiveLayoutConfig, area: Rect) {
    let progress = self
      .current_progress()
      .unwrap_or(self.focused_page as f64 + 0.5);
    let rebuild = self.follower.as_ref().is_none_or(|panel| {
      panel.config != config || panel.area != area || panel.cell_pixels != self.terminal_cell_pixels
    });
    if rebuild {
      let scroll_layout = config.is_scroll().then(|| {
        layout::compute_scroll_layout(
          self.document.page_count,
          area.width,
          area.height,
          &config,
          |index| self.page_dimensions(index),
          self.terminal_cell_pixels,
        )
      });
      self.follower = Some(FollowerPanel {
        config,
        area,
        scroll_layout,
        start: 0,
        cell_pixels: self.terminal_cell_pixels,
        progress: None,
      });
    }
    let panel = self.follower.as_mut().unwrap();
    if panel.progress == Some(progress) {
      return;
    }
    panel.start = if let Some(layout) = &panel.scroll_layout {
      best_scroll_row_for_progress(layout, area.height, panel.config.scroll_divisor, progress)
    } else {
      best_grid_start_for_progress(
        progress,
        panel.config.grid_capacity(),
        panel.config.columns as usize,
        self.document.page_count,
      )
    };
    panel.progress = Some(progress);
  }

  pub(super) fn follower_page_at(&self, column: u16, row: u16) -> Option<usize> {
    let panel = self.follower.as_ref()?;
    if !crate::geometry::contains(panel.area, column, row) {
      return None;
    }
    if let Some(layout) = &panel.scroll_layout {
      layout
        .placed_items(panel.start, panel.area, panel.config.scroll_divisor)
        .into_iter()
        .find(|placed| {
          crate::geometry::contains(placed.area, column, row)
            || crate::geometry::contains(placed.label_area, column, row)
        })
        .map(|placed| placed.item.page_index)
    } else {
      layout::grid_slots(panel.area, &panel.config)
        .iter()
        .position(|slot| crate::geometry::contains(*slot, column, row))
        .map(|index| panel.start + index)
        .filter(|index| *index < self.document.page_count)
    }
  }
}
