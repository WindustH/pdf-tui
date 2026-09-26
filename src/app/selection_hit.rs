//! Mapping mouse positions to page coordinates in the current view: the
//! page areas drawn by the viewer (grid slots or scroll slices) or the
//! selection view's crop.

use crate::{
  geometry::{contains, fitted_page_area, slot_content_area},
  layout,
  selection::{PdfPoint, PdfSelection, SelectionAnchor},
};

use super::{
  App, ViewMode,
  selection_geometry::{
    PageDisplay, SelectionHit, anchor_at_point, anchor_from_hit, bounded_opposite_point,
    distance_to_rect, hit_for_display, hit_for_selection_display, page_display_for_area,
    projected_point_for_display, projected_point_for_selection_display,
  },
  selection_state::SelectionDisplay,
};

impl App {
  pub(super) fn viewer_hit_at(&self, column: u16, row: u16) -> Option<SelectionHit> {
    self
      .viewer_page_displays()
      .into_iter()
      .find(|display| contains(display.area, column, row))
      .map(|display| hit_for_display(display, column, row, false))
  }

  pub(super) fn selection_hit_for_current_view(
    &self,
    column: u16,
    row: u16,
    clamp_to_display: bool,
  ) -> Option<SelectionHit> {
    match self.view {
      ViewMode::Viewer => {
        if clamp_to_display {
          return None;
        }
        self.viewer_hit_at(column, row)
      }
      ViewMode::Selection => self.selection_view_hit_at(column, row, clamp_to_display),
      _ => None,
    }
  }

  pub(super) fn selection_view_hit_at(
    &self,
    column: u16,
    row: u16,
    clamp_to_display: bool,
  ) -> Option<SelectionHit> {
    let display = self.current_selection_display()?;
    if !clamp_to_display && !contains(display.area, column, row) {
      return None;
    }
    Some(hit_for_selection_display(
      display,
      column,
      row,
      clamp_to_display,
    ))
  }

  pub(super) fn current_selection_display(&self) -> Option<&SelectionDisplay> {
    let display = self.selection.display.as_ref()?;
    (self.selection.index == Some(display.selection_index)).then_some(display)
  }

  pub(super) fn anchor_for_click_on_page(
    &self,
    reference: SelectionAnchor,
    column: u16,
    row: u16,
    bounds: Option<PdfSelection>,
  ) -> Option<SelectionAnchor> {
    let inside = self
      .selection_hit_for_current_view(column, row, false)
      .filter(|hit| hit.page_index == reference.page_index)
      .map(anchor_from_hit);
    if let Some(anchor) = inside {
      return Some(anchor);
    }
    let point = self.projected_point_for_current_view(reference.page_index, column, row)?;
    let endpoint = bounded_opposite_point(reference.point, point, reference, bounds)?;
    Some(anchor_at_point(
      reference.page_index,
      reference.page_width,
      reference.page_height,
      endpoint,
      reference.marker.width().max(1.0),
      reference.marker.height().max(1.0),
      bounds.map(|selection| selection.rect),
    ))
  }

  pub(super) fn anchor_for_click_relative_to(
    &self,
    reference: SelectionAnchor,
    clicked: SelectionAnchor,
    bounds: Option<PdfSelection>,
  ) -> SelectionAnchor {
    let point = bounded_opposite_point(reference.point, clicked.point, reference, bounds)
      .unwrap_or(clicked.point);
    anchor_at_point(
      reference.page_index,
      reference.page_width,
      reference.page_height,
      point,
      reference
        .marker
        .width()
        .max(clicked.marker.width())
        .max(1.0),
      reference
        .marker
        .height()
        .max(clicked.marker.height())
        .max(1.0),
      bounds.map(|selection| selection.rect),
    )
  }

  pub(super) fn projected_point_for_current_view(
    &self,
    page_index: usize,
    column: u16,
    row: u16,
  ) -> Option<PdfPoint> {
    match self.view {
      ViewMode::Viewer => self
        .viewer_page_displays()
        .into_iter()
        .filter(|display| display.page_index == page_index)
        .min_by_key(|display| distance_to_rect(display.area, column, row))
        .map(|display| projected_point_for_display(display, column, row)),
      ViewMode::Selection => {
        let display = self.current_selection_display()?;
        (display.page_index == page_index)
          .then(|| projected_point_for_selection_display(display, column, row))
      }
      _ => None,
    }
  }

  pub(super) fn viewer_page_displays(&self) -> Vec<PageDisplay> {
    if self.layout.is_scroll() {
      self.scroll_page_displays()
    } else {
      self.grid_page_displays()
    }
  }

  pub(super) fn grid_page_displays(&self) -> Vec<PageDisplay> {
    let Some(viewport) = self.viewport else {
      return Vec::new();
    };
    layout::grid_slots(viewport, &self.layout)
      .into_iter()
      .enumerate()
      .filter_map(|(slot_index, slot)| {
        let page_index = self.grid_start_page.saturating_add(slot_index);
        if page_index >= self.document.page_count {
          return None;
        }
        let image_area = fitted_page_area(
          slot_content_area(slot, &self.layout),
          self.terminal_cell_pixels,
          self.page_dimensions(page_index),
        );
        page_display_for_area(self, page_index, image_area, image_area.height, 0)
      })
      .collect()
  }

  pub(super) fn scroll_page_displays(&self) -> Vec<PageDisplay> {
    let (Some(viewport), Some(cached)) = (self.viewport, self.scroll_layout.as_ref()) else {
      return Vec::new();
    };
    cached
      .layout()
      .placed_items(self.scroll as usize, viewport, cached.scroll_divisor())
      .into_iter()
      .filter_map(|placed| {
        let item = placed.item;
        let (y_cell_start, _) = layout::grid_slice_span(
          item.grid_height,
          item.slice_count,
          item.slice_index,
          item.full_height,
        );
        let y_cell_start = y_cell_start.min(u32::from(u16::MAX)) as u16;
        let display = page_display_for_area(
          self,
          item.page_index,
          placed.area,
          item.full_height,
          y_cell_start,
        )?;
        Some(PageDisplay {
          full_cell_width: item.full_width,
          ..display
        })
      })
      .collect()
  }
}
