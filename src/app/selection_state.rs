use crossterm::event::MouseEvent;
use ratatui::layout::Rect;
use tokio::sync::mpsc;

use crate::{
  clipboard,
  event::{AsyncEvent, ClipboardKind, ClipboardOutcome, SelectionImageOutcome},
  layout,
  selection::{self, PdfPoint, PdfRect, PdfSelection, SelectionAnchor},
};

use super::selection_geometry::{
  PageDisplay, SelectionHit, anchor_at_point, anchor_from_hit, bounded_opposite_point, contains,
  distance_to_rect, fitted_page_area, hit_for_display, hit_for_selection_display,
  normalized_distance, page_display_for_area, projected_point_for_display,
  projected_point_for_selection_display, safe_inner,
};
use super::{App, SelectionDisplay, SelectionMousePress, ViewMode};

impl App {
  pub(super) fn enter_selection_view(&mut self) {
    self.commit_selection_draft();
    self.view = ViewMode::Selection;
    self.clear_frame_navigation_lock();
    self.key_dispatcher.clear();
    if self.selection_index.is_none() && !self.selections.is_empty() {
      self.selection_index = Some(self.selections.len().saturating_sub(1));
    }
    if let Some(selection) = self.current_selection().copied() {
      self.focused_page = selection
        .page_index
        .min(self.document.page_count.saturating_sub(1));
      self.set_message(format!(
        "selection {}/{} on page {}",
        self.selection_index.unwrap_or(0) + 1,
        self.selections.len(),
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
    let Some(anchor) = self.selection_anchor else {
      let Some(hit) = self.selection_hit_for_current_view(mouse.column, mouse.row, false) else {
        return;
      };
      self.selection_anchor = Some(SelectionAnchor {
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

    if let Some(second) = self.selection_second_anchor {
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
        self.selection_anchor =
          Some(self.anchor_for_click_relative_to(second, endpoint, selection_bounds));
      } else {
        self.selection_second_anchor =
          Some(self.anchor_for_click_relative_to(anchor, endpoint, selection_bounds));
      }
    } else {
      self.selection_second_anchor = Some(endpoint);
    }

    self.upsert_selection_draft(selection_bounds);
  }

  pub(super) fn begin_selection_mouse_press(&mut self, mouse: MouseEvent) {
    self.selection_mouse_press = None;
    if self.selection_anchor.is_some() || self.selection_second_anchor.is_some() {
      return;
    }
    let Some(hit) = self.selection_hit_for_current_view(mouse.column, mouse.row, false) else {
      return;
    };
    self.selection_anchor = Some(SelectionAnchor {
      page_index: hit.page_index,
      page_width: hit.page_width,
      page_height: hit.page_height,
      point: hit.point,
      marker: hit.cell_rect,
    });
    self.selection_mouse_press = Some(SelectionMousePress {
      column: mouse.column,
      row: mouse.row,
      saw_drag: false,
    });
    self.key_dispatcher.clear();
    self.set_message(format!("selection anchor: page {}", hit.page_index + 1));
  }

  pub(super) fn handle_selection_mouse_drag(&mut self, mouse: MouseEvent) {
    if let Some(press) = &mut self.selection_mouse_press
      && (press.column != mouse.column || press.row != mouse.row)
    {
      press.saw_drag = true;
    }
  }

  pub(super) fn finish_selection_mouse_press(
    &mut self,
    mouse: MouseEvent,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) -> bool {
    let Some(press) = self.selection_mouse_press.take() else {
      return false;
    };
    if self.selection_anchor.is_none() {
      return true;
    }
    let moved = press.saw_drag || press.column != mouse.column || press.row != mouse.row;
    if moved {
      self.handle_selection_mouse_click(mouse, tx);
    }
    true
  }

  pub(super) fn cancel_selection_anchor(&mut self) {
    if self.selection_anchor.is_none() && self.selection_second_anchor.is_none() {
      return;
    }
    self.remove_selection_draft();
    self.selection_anchor = None;
    self.selection_second_anchor = None;
    self.selection_mouse_press = None;
    self.key_dispatcher.clear();
    self.set_message("selection cancelled");
  }

  pub fn selection_markers_for(&self, page_index: usize) -> Vec<PdfRect> {
    [self.selection_anchor, self.selection_second_anchor]
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
      .selection_index
      .and_then(|index| self.selections.get(index))
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
    if self.selection_draft_index.is_some() {
      if self.commit_selection_draft().is_some()
        && let Some(selection) = self.current_selection().copied()
      {
        self.selection_display = None;
        self.focused_page = selection
          .page_index
          .min(self.document.page_count.saturating_sub(1));
        self.set_message(format!(
          "selection {}/{} on page {}",
          self.selection_index.unwrap_or(0) + 1,
          self.selections.len(),
          selection.page_index + 1
        ));
      }
      return;
    }
    if self.selection_anchor.is_some() {
      self.set_message("click inside current selection to mark opposite anchor");
      return;
    }
    self.key_dispatcher.clear();
    self.set_message("click inside current selection to mark anchor");
  }

  pub(super) fn selection_anchor_state(&self) -> Option<String> {
    let first = self.selection_anchor?;
    Some(format!(
      "{:.3},{:.3},{:.3},{:.3}:{:?}:{:?}",
      first.marker.x_min,
      first.marker.y_min,
      first.marker.x_max,
      first.marker.y_max,
      self.selection_second_anchor.map(|anchor| (
        anchor.marker.x_min,
        anchor.marker.y_min,
        anchor.marker.x_max,
        anchor.marker.y_max
      )),
      self.selection_draft_index
    ))
  }

  pub(super) fn selection_copy_text(&mut self, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    if self.selection_copy_text_pending {
      return;
    }
    if self.current_selection().is_none() {
      self.set_message("no selection");
      return;
    }
    if self.search_index.is_none() {
      self.selection_copy_text_pending = true;
      self.request_search_index(tx);
      self.set_message("building text index for selection...");
      return;
    }
    self.copy_selection_text_from_ready_index(tx);
  }

  pub fn finish_pending_selection_text_copy(&mut self, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    if !self.selection_copy_text_pending {
      return;
    }
    self.selection_copy_text_pending = false;
    self.copy_selection_text_from_ready_index(tx);
  }

  pub(super) fn selection_copy_image(&mut self, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    if self.selection_copy_image_pending {
      return;
    }
    let Some(selection) = self.current_selection().copied() else {
      self.set_message("no selection");
      return;
    };
    let document = self.document.clone();
    let cache_dir = self.settings.cache_dir.clone();
    let max_pixels = self.settings.config.render.selection_image_max_pixels;
    let cache_max_bytes = self.settings.config.render.selection_cache_max_bytes;
    let tx = tx.clone();
    self.selection_copy_image_pending = true;
    self.set_message("copying selected image...");
    tokio::spawn(async move {
      let result = selection::render_selection_copy_image(
        document,
        selection,
        cache_dir,
        max_pixels,
        cache_max_bytes,
      )
      .await;
      let result = match result {
        Ok(path) => match clipboard::copy_png(&path).await {
          Ok(()) => Ok(format!("copied selected image: {}", path.display())),
          Err(error) => Err(error),
        },
        Err(error) => Err(error),
      };
      let _ = tx.send(AsyncEvent::Clipboard(ClipboardOutcome {
        kind: ClipboardKind::SelectionImage,
        result,
      }));
    });
  }

  pub fn request_selection_image(
    &mut self,
    selection: PdfSelection,
    target_width: u32,
    target_height: u32,
    preload: bool,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) -> String {
    let target_width = target_width.max(1);
    let target_height = target_height.max(1);
    let key = selection::selection_image_cache_key(
      &self.document,
      selection,
      target_width,
      target_height,
      "preview",
    );
    if self.selection_images.contains_key(&key)
      || self.selection_image_errors.contains_key(&key)
      || self.selection_image_in_flight.contains(&key)
    {
      return key;
    }
    if selection.page_index >= self.document.page_count {
      self.selection_image_errors.insert(
        key.clone(),
        format!("page {} is outside the document", selection.page_index + 1),
      );
      return key;
    }
    self.selection_image_in_flight.insert(key.clone());
    let document = self.document.clone();
    let source_size_bytes = document.size_bytes;
    let source_modified_nanos = document.modified_nanos;
    let cache_dir = self.settings.cache_dir.clone();
    let cache_max_bytes = self.settings.config.render.selection_cache_max_bytes;
    let key_for_task = key.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
      let result = selection::render_selection_preview_image(
        document,
        selection,
        cache_dir,
        target_width,
        target_height,
        cache_max_bytes,
      )
      .await;
      let _ = tx.send(AsyncEvent::SelectionImage(SelectionImageOutcome {
        source_size_bytes,
        source_modified_nanos,
        key: key_for_task,
        preload,
        result,
      }));
    });
    key
  }

  pub fn set_selection_display(
    &mut self,
    selection_index: usize,
    selection: PdfSelection,
    area: Rect,
  ) {
    self.selection_display = Some(SelectionDisplay {
      selection_index,
      page_index: selection.page_index,
      page_width: selection.page_width,
      page_height: selection.page_height,
      rect: selection.rect,
      area,
    });
  }

  pub fn clear_selection_display(&mut self) {
    self.selection_display = None;
  }

  pub fn finish_clipboard(&mut self, outcome: ClipboardOutcome) {
    match outcome.kind {
      ClipboardKind::SelectionText => self.selection_copy_text_pending = false,
      ClipboardKind::SelectionImage => self.selection_copy_image_pending = false,
    }
    match outcome.result {
      Ok(message) => self.set_message(message),
      Err(error) => self.set_message(error),
    }
  }

  fn copy_selection_text_from_ready_index(&mut self, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    let Some(selection) = self.current_selection().copied() else {
      self.set_message("no selection");
      return;
    };
    let Some(index) = &self.search_index else {
      let message = self
        .search_index_error
        .as_ref()
        .map(|error| format!("selection text unavailable: {error}"))
        .unwrap_or_else(|| "selection text index is not ready".to_string());
      self.set_message(message);
      return;
    };
    let text = index.text_in_selection(selection);
    let chars = text.chars().count();
    let tx = tx.clone();
    self.selection_copy_text_pending = true;
    self.set_message("copying selected text...");
    tokio::spawn(async move {
      let result = clipboard::copy_text(text)
        .await
        .map(|()| format!("copied selected text: {chars} chars"));
      let _ = tx.send(AsyncEvent::Clipboard(ClipboardOutcome {
        kind: ClipboardKind::SelectionText,
        result,
      }));
    });
  }

  fn select_selection_delta(&mut self, delta: isize) {
    if self.commit_selection_draft().is_some() {
      self.selection_display = None;
      if let Some(selection) = self.current_selection().copied() {
        self.focused_page = selection
          .page_index
          .min(self.document.page_count.saturating_sub(1));
        self.set_message(format!(
          "selection {}/{} on page {}",
          self.selection_index.unwrap_or(0) + 1,
          self.selections.len(),
          selection.page_index + 1
        ));
      }
      return;
    }
    self.selection_display = None;
    if self.selections.is_empty() {
      self.selection_index = None;
      self.set_message("no selection history");
      return;
    }
    let current = self.selection_index.unwrap_or(0);
    let next = current
      .saturating_add_signed(delta)
      .min(self.selections.len().saturating_sub(1));
    self.selection_index = Some(next);
    if let Some(selection) = self.current_selection().copied() {
      self.focused_page = selection
        .page_index
        .min(self.document.page_count.saturating_sub(1));
      self.set_message(format!(
        "selection {}/{} on page {}",
        next + 1,
        self.selections.len(),
        selection.page_index + 1
      ));
    }
  }

  fn viewer_hit_at(&self, column: u16, row: u16) -> Option<SelectionHit> {
    self
      .viewer_page_displays()
      .into_iter()
      .find(|display| contains(display.area, column, row))
      .map(|display| hit_for_display(display, column, row, false))
  }

  fn selection_hit_for_current_view(
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

  fn selection_view_hit_at(
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

  fn current_selection_display(&self) -> Option<&SelectionDisplay> {
    let display = self.selection_display.as_ref()?;
    (self.selection_index == Some(display.selection_index)).then_some(display)
  }

  fn selection_bounds_for_current_view(&self) -> Option<PdfSelection> {
    (self.view == ViewMode::Selection)
      .then(|| self.current_selection().copied())
      .flatten()
  }

  fn anchor_for_click_on_page(
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

  fn anchor_for_click_relative_to(
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

  fn projected_point_for_current_view(
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

  fn upsert_selection_draft(&mut self, bounds: Option<PdfSelection>) {
    let Some(selection) = self.selection_from_anchors(bounds) else {
      self.set_message("selection is empty");
      return;
    };
    let index = if let Some(index) = self
      .selection_draft_index
      .filter(|index| *index < self.selections.len())
    {
      self.selections[index] = selection;
      index
    } else {
      let index = self.selection_draft_insert_index();
      self.selections.insert(index, selection);
      if let Some(selected) = self.selection_index
        && selected >= index
        && self.view != ViewMode::Selection
      {
        self.selection_index = Some(selected.saturating_add(1));
      }
      self.selection_draft_index = Some(index);
      index
    };
    if self.view == ViewMode::Viewer {
      self.selection_index = Some(index);
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
    let first = self.selection_anchor?;
    let second = self.selection_second_anchor?;
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
      && let Some(parent) = self.selection_index
    {
      return parent.saturating_add(1).min(self.selections.len());
    }
    self.selections.len()
  }

  fn commit_selection_draft(&mut self) -> Option<usize> {
    let committed = if let Some(index) = self
      .selection_draft_index
      .take()
      .filter(|index| *index < self.selections.len())
    {
      self.selection_index = Some(index);
      self.selection_display = None;
      self.selection_images.clear();
      self.selection_image_errors.clear();
      self.selection_image_in_flight.clear();
      Some(index)
    } else {
      None
    };
    self.selection_anchor = None;
    self.selection_second_anchor = None;
    self.selection_mouse_press = None;
    committed
  }

  fn remove_selection_draft(&mut self) {
    let Some(index) = self.selection_draft_index.take() else {
      return;
    };
    if index >= self.selections.len() {
      return;
    }
    self.selections.remove(index);
    if let Some(selected) = self.selection_index {
      if selected == index {
        self.selection_index = None;
      } else if selected > index {
        self.selection_index = Some(selected - 1);
      }
    }
  }

  fn viewer_page_displays(&self) -> Vec<PageDisplay> {
    if self.layout.is_scroll() {
      self.scroll_page_displays()
    } else {
      self.grid_page_displays()
    }
  }

  fn grid_page_displays(&self) -> Vec<PageDisplay> {
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
        let page_area = if self.layout.show_border {
          safe_inner(
            slot,
            self.layout.padding.saturating_add(1),
            self.layout.padding.saturating_add(1),
          )
        } else {
          safe_inner(slot, self.layout.padding, self.layout.padding)
        };
        let image_area = fitted_page_area(
          page_area,
          self.terminal_cell_pixels,
          self.page_dimensions(page_index),
        );
        page_display_for_area(self, page_index, image_area, image_area.height, 0)
      })
      .collect()
  }

  fn scroll_page_displays(&self) -> Vec<PageDisplay> {
    let Some(viewport) = self.viewport else {
      return Vec::new();
    };
    let Some(scroll_layout) = self.last_scroll_layout.as_ref() else {
      return Vec::new();
    };
    let visible_rows = layout::visible_scroll_rows(
      scroll_layout,
      self.scroll as usize,
      viewport.height,
      self.layout.scroll_divisor,
    );
    let used_height = layout::visible_rows_height(scroll_layout, &visible_rows);
    let mut row_y = viewport
      .y
      .saturating_add(viewport.height.saturating_sub(used_height) / 2);
    let mut displays = Vec::new();
    for (position, row_index) in visible_rows.iter().copied().enumerate() {
      let Some(row) = scroll_layout.rows.get(row_index) else {
        continue;
      };
      for item_index in &row.items {
        let Some(item) = scroll_layout.items.get(*item_index).copied() else {
          continue;
        };
        let item_y = row_y.saturating_add(row.height.saturating_sub(item.height) / 2);
        let area = Rect::new(
          viewport.x.saturating_add(item.x),
          item_y,
          item.width.min(viewport.width.saturating_sub(item.x)),
          item.height,
        );
        let y_cell_start = ((u32::from(item.full_height) * u32::from(item.slice_index))
          / u32::from(item.slice_count.max(1)))
        .min(u32::from(u16::MAX)) as u16;
        if let Some(display) =
          page_display_for_area(self, item.page_index, area, item.full_height, y_cell_start)
        {
          displays.push(PageDisplay {
            full_cell_width: item.full_width,
            ..display
          });
        }
      }
      row_y = row_y.saturating_add(row.height);
      if position + 1 < visible_rows.len() {
        row_y = row_y.saturating_add(row.gap_after);
      }
    }
    displays
  }
}
