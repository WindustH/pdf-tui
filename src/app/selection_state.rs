//! Rectangular selections: the anchors of the selection being made, the
//! session history, and the mouse and key actions that edit them.

use std::collections::{HashMap, HashSet};

use crossterm::event::{MouseButton, MouseEvent};
use ratatui::layout::Rect;
use tokio::sync::mpsc;

use crate::{
  event::AsyncEvent,
  pdf::PageImage,
  selection::{PdfRect, PdfSelection, SelectionAnchor},
};

use super::{App, ViewMode, selection_geometry::normalized_distance};

/// Where the current selection is drawn in the selection view, so clicks
/// there can be mapped back to page coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectionDisplay {
  pub selection_index: usize,
  pub page_index: usize,
  pub page_width: f64,
  pub page_height: f64,
  pub rect: PdfRect,
  pub area: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SelectionMousePress {
  button: MouseButton,
  column: u16,
  row: u16,
  saw_drag: bool,
}

/// Selections made this session, the anchors of the one being made, and
/// the preview crops rendered for them.
#[derive(Debug, Default)]
pub struct SelectionState {
  pub anchor: Option<SelectionAnchor>,
  pub second_anchor: Option<SelectionAnchor>,
  mouse_press: Option<SelectionMousePress>,
  /// Index in `history` of the selection being edited, until committed.
  pub(super) draft_index: Option<usize>,
  pub display: Option<SelectionDisplay>,
  pub images: HashMap<String, PageImage>,
  pub image_errors: HashMap<String, String>,
  pub(super) image_in_flight: HashSet<String>,
  pub history: Vec<PdfSelection>,
  pub index: Option<usize>,
  pub(super) copy_text_pending: bool,
  pub(super) copy_image_pending: bool,
}

impl SelectionState {
  /// Drops the anchors of a selection in progress when leaving a view. An
  /// uncommitted draft stays in the history as a regular selection.
  pub(super) fn leave_view(&mut self) {
    self.anchor = None;
    self.second_anchor = None;
    self.mouse_press = None;
    self.draft_index = None;
    self.display = None;
  }

  pub(super) fn clear_images(&mut self) {
    self.images.clear();
    self.image_errors.clear();
    self.image_in_flight.clear();
  }
}

impl App {
  pub(super) fn enter_selection_view(&mut self) {
    self.commit_selection_draft();
    self.view = ViewMode::Selection;
    self.clear_frame_navigation_lock();
    self.key_dispatcher.clear();
    if self.selection.index.is_none() && !self.selection.history.is_empty() {
      self.selection.index = Some(self.selection.history.len().saturating_sub(1));
    }
    if let Some(selection) = self.current_selection().copied() {
      self.focused_page = selection
        .page_index
        .min(self.document.page_count.saturating_sub(1));
      self.set_message(format!(
        "selection {}/{} on page {}",
        self.selection.index.unwrap_or(0) + 1,
        self.selection.history.len(),
        selection.page_index + 1
      ));
    } else {
      self.set_message("no selection");
    }
  }

  pub(super) fn handle_selection_mouse_click(
    &mut self,
    mouse: MouseEvent,
    _tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) {
    let selection_bounds = self.selection_bounds_for_current_view();
    let Some(anchor) = self.selection.anchor else {
      let Some(hit) = self.selection_hit_for_current_view(mouse.column, mouse.row, false) else {
        return;
      };
      self.selection.anchor = Some(SelectionAnchor {
        page_index: hit.page_index,
        page_width: hit.page_width,
        page_height: hit.page_height,
        point: hit.point,
        marker: hit.cell_rect,
      });
      self.key_dispatcher.clear();
      self.set_message(format!("selection anchor: page {}", hit.page_index + 1));
      return;
    };

    let Some(endpoint) =
      self.anchor_for_click_on_page(anchor, mouse.column, mouse.row, selection_bounds)
    else {
      self.set_message("selection endpoint is outside the anchor page");
      return;
    };

    if let Some(second) = self.selection.second_anchor {
      if normalized_distance(
        endpoint.point,
        anchor.point,
        anchor.page_width,
        anchor.page_height,
      ) <= normalized_distance(
        endpoint.point,
        second.point,
        second.page_width,
        second.page_height,
      ) {
        self.selection.anchor =
          Some(self.anchor_for_click_relative_to(second, endpoint, selection_bounds));
      } else {
        self.selection.second_anchor =
          Some(self.anchor_for_click_relative_to(anchor, endpoint, selection_bounds));
      }
    } else {
      self.selection.second_anchor = Some(endpoint);
    }

    self.upsert_selection_draft(selection_bounds);
  }

  pub(super) fn begin_selection_mouse_press(&mut self, mouse: MouseEvent, button: MouseButton) {
    self.selection.mouse_press = None;
    if self.selection.anchor.is_some() || self.selection.second_anchor.is_some() {
      return;
    }
    let Some(hit) = self.selection_hit_for_current_view(mouse.column, mouse.row, false) else {
      return;
    };
    self.selection.anchor = Some(SelectionAnchor {
      page_index: hit.page_index,
      page_width: hit.page_width,
      page_height: hit.page_height,
      point: hit.point,
      marker: hit.cell_rect,
    });
    self.selection.mouse_press = Some(SelectionMousePress {
      button,
      column: mouse.column,
      row: mouse.row,
      saw_drag: false,
    });
    self.key_dispatcher.clear();
    self.set_message(format!("selection anchor: page {}", hit.page_index + 1));
  }

  pub(super) fn handle_selection_mouse_drag(&mut self, mouse: MouseEvent, button: MouseButton) {
    if let Some(press) = &mut self.selection.mouse_press
      && press.button == button
      && (press.column != mouse.column || press.row != mouse.row)
    {
      press.saw_drag = true;
    }
  }

  pub(super) fn finish_selection_mouse_press(
    &mut self,
    mouse: MouseEvent,
    button: MouseButton,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) -> bool {
    let Some(press) = self.selection.mouse_press else {
      return false;
    };
    if press.button != button {
      return false;
    }
    self.selection.mouse_press = None;
    if self.selection.anchor.is_none() {
      return true;
    }
    let moved = press.saw_drag || press.column != mouse.column || press.row != mouse.row;
    if moved {
      self.handle_selection_mouse_click(mouse, tx);
    }
    true
  }

  pub(super) fn cancel_selection_anchor(&mut self) {
    if self.selection.anchor.is_none() && self.selection.second_anchor.is_none() {
      return;
    }
    self.remove_selection_draft();
    self.selection.anchor = None;
    self.selection.second_anchor = None;
    self.selection.mouse_press = None;
    self.key_dispatcher.clear();
    self.set_message("selection cancelled");
  }

  pub fn selection_markers_for(&self, page_index: usize) -> Vec<PdfRect> {
    [self.selection.anchor, self.selection.second_anchor]
      .into_iter()
      .flatten()
      .filter(|anchor| anchor.page_index == page_index)
      .map(|anchor| anchor.marker)
      .collect()
  }

  pub fn selection_draft_outline_for(&self, page_index: usize) -> Option<PdfRect> {
    let selection = self.selection_from_anchors(self.selection_bounds_for_current_view())?;
    (selection.page_index == page_index).then_some(selection.rect)
  }

  pub fn current_selection(&self) -> Option<&PdfSelection> {
    self
      .selection
      .index
      .and_then(|index| self.selection.history.get(index))
  }

  pub(super) fn selection_next(&mut self) {
    self.select_selection_delta(1);
  }

  pub(super) fn selection_previous(&mut self) {
    self.select_selection_delta(-1);
  }

  pub(super) fn selection_reselect(&mut self) {
    if self.current_selection().is_none() {
      self.set_message("no selection");
      return;
    }
    if self.selection.draft_index.is_some() {
      if self.commit_selection_draft().is_some()
        && let Some(selection) = self.current_selection().copied()
      {
        self.selection.display = None;
        self.focused_page = selection
          .page_index
          .min(self.document.page_count.saturating_sub(1));
        self.set_message(format!(
          "selection {}/{} on page {}",
          self.selection.index.unwrap_or(0) + 1,
          self.selection.history.len(),
          selection.page_index + 1
        ));
      }
      return;
    }
    if self.selection.anchor.is_some() {
      self.set_message("click inside current selection to mark opposite anchor");
      return;
    }
    self.key_dispatcher.clear();
    self.set_message("click inside current selection to mark anchor");
  }

  pub(super) fn selection_anchor_state(&self) -> Option<String> {
    let first = self.selection.anchor?;
    Some(format!(
      "{:.3},{:.3},{:.3},{:.3}:{:?}:{:?}",
      first.marker.x_min,
      first.marker.y_min,
      first.marker.x_max,
      first.marker.y_max,
      self.selection.second_anchor.map(|anchor| (
        anchor.marker.x_min,
        anchor.marker.y_min,
        anchor.marker.x_max,
        anchor.marker.y_max
      )),
      self.selection.draft_index
    ))
  }

  pub fn set_selection_display(
    &mut self,
    selection_index: usize,
    selection: PdfSelection,
    area: Rect,
  ) {
    self.selection.display = Some(SelectionDisplay {
      selection_index,
      page_index: selection.page_index,
      page_width: selection.page_width,
      page_height: selection.page_height,
      rect: selection.rect,
      area,
    });
  }

  pub fn clear_selection_display(&mut self) {
    self.selection.display = None;
  }

  fn select_selection_delta(&mut self, delta: isize) {
    if self.commit_selection_draft().is_some() {
      self.selection.display = None;
      if let Some(selection) = self.current_selection().copied() {
        self.focused_page = selection
          .page_index
          .min(self.document.page_count.saturating_sub(1));
        self.set_message(format!(
          "selection {}/{} on page {}",
          self.selection.index.unwrap_or(0) + 1,
          self.selection.history.len(),
          selection.page_index + 1
        ));
      }
      return;
    }
    self.selection.display = None;
    if self.selection.history.is_empty() {
      self.selection.index = None;
      self.set_message("no selection history");
      return;
    }
    let current = self.selection.index.unwrap_or(0);
    let next = current
      .saturating_add_signed(delta)
      .min(self.selection.history.len().saturating_sub(1));
    self.selection.index = Some(next);
    if let Some(selection) = self.current_selection().copied() {
      self.focused_page = selection
        .page_index
        .min(self.document.page_count.saturating_sub(1));
      self.set_message(format!(
        "selection {}/{} on page {}",
        next + 1,
        self.selection.history.len(),
        selection.page_index + 1
      ));
    }
  }

  fn selection_bounds_for_current_view(&self) -> Option<PdfSelection> {
    (self.view == ViewMode::Selection)
      .then(|| self.current_selection().copied())
      .flatten()
  }

  fn upsert_selection_draft(&mut self, bounds: Option<PdfSelection>) {
    let Some(selection) = self.selection_from_anchors(bounds) else {
      self.set_message("selection is empty");
      return;
    };
    let index = if let Some(index) = self
      .selection
      .draft_index
      .filter(|index| *index < self.selection.history.len())
    {
      self.selection.history[index] = selection;
      index
    } else {
      let index = self.selection_draft_insert_index();
      self.selection.history.insert(index, selection);
      if let Some(selected) = self.selection.index
        && selected >= index
        && self.view != ViewMode::Selection
      {
        self.selection.index = Some(selected.saturating_add(1));
      }
      self.selection.draft_index = Some(index);
      index
    };
    if self.view == ViewMode::Viewer {
      self.selection.index = Some(index);
    }
    self.focused_page = selection
      .page_index
      .min(self.document.page_count.saturating_sub(1));
    self.key_dispatcher.clear();
    self.set_message(format!(
      "selection draft {} on page {}",
      index + 1,
      selection.page_index + 1
    ));
  }

  fn selection_from_anchors(&self, bounds: Option<PdfSelection>) -> Option<PdfSelection> {
    let first = self.selection.anchor?;
    let second = self.selection.second_anchor?;
    if first.page_index != second.page_index {
      return None;
    }
    let mut rect = PdfRect {
      x_min: first.point.x.min(second.point.x),
      y_min: first.point.y.min(second.point.y),
      x_max: first.point.x.max(second.point.x),
      y_max: first.point.y.max(second.point.y),
    }
    .clamp_to_page(first.page_width, first.page_height);
    if let Some(bounds) = bounds {
      rect = rect.intersection(bounds.rect)?;
    }
    (!rect.is_empty()).then_some(PdfSelection {
      page_index: first.page_index,
      page_width: first.page_width,
      page_height: first.page_height,
      rect,
    })
  }

  fn selection_draft_insert_index(&self) -> usize {
    if self.view == ViewMode::Selection
      && let Some(parent) = self.selection.index
    {
      return parent.saturating_add(1).min(self.selection.history.len());
    }
    self.selection.history.len()
  }

  fn commit_selection_draft(&mut self) -> Option<usize> {
    let committed = if let Some(index) = self
      .selection
      .draft_index
      .take()
      .filter(|index| *index < self.selection.history.len())
    {
      self.selection.index = Some(index);
      self.selection.display = None;
      self.selection.images.clear();
      self.selection.image_errors.clear();
      self.selection.image_in_flight.clear();
      Some(index)
    } else {
      None
    };
    self.selection.anchor = None;
    self.selection.second_anchor = None;
    self.selection.mouse_press = None;
    committed
  }

  fn remove_selection_draft(&mut self) {
    let Some(index) = self.selection.draft_index.take() else {
      return;
    };
    if index >= self.selection.history.len() {
      return;
    }
    self.selection.history.remove(index);
    if let Some(selected) = self.selection.index {
      if selected == index {
        self.selection.index = None;
      } else if selected > index {
        self.selection.index = Some(selected - 1);
      }
    }
  }
}
