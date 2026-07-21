use crossterm::event::MouseEvent;
use ratatui::layout::{Margin, Rect};
use tokio::sync::mpsc;

use crate::{
  clipboard,
  event::{AsyncEvent, ClipboardKind, ClipboardOutcome, SelectionImageOutcome},
  layout,
  selection::{self, PdfPoint, PdfRect, PdfSelection, SelectionAnchor},
};

use super::{App, SelectionDisplay, ViewMode};

#[derive(Debug, Clone, Copy)]
struct SelectionHit {
  page_index: usize,
  page_width: f64,
  page_height: f64,
  point: PdfPoint,
  cell_rect: PdfRect,
}

#[derive(Debug, Clone, Copy)]
struct PageDisplay {
  page_index: usize,
  page_width: f64,
  page_height: f64,
  area: Rect,
  full_cell_width: u16,
  full_cell_height: u16,
  y_cell_start: u16,
}

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

  pub(super) fn cancel_selection_anchor(&mut self) {
    if self.selection_anchor.is_none() && self.selection_second_anchor.is_none() {
      return;
    }
    self.remove_selection_draft();
    self.selection_anchor = None;
    self.selection_second_anchor = None;
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

fn page_display_for_area(
  app: &App,
  page_index: usize,
  area: Rect,
  full_cell_height: u16,
  y_cell_start: u16,
) -> Option<PageDisplay> {
  if area.width == 0 || area.height == 0 {
    return None;
  }
  let (page_width, page_height) = app.page_dimensions(page_index)?;
  Some(PageDisplay {
    page_index,
    page_width: f64::from(page_width.max(1)),
    page_height: f64::from(page_height.max(1)),
    area,
    full_cell_width: area.width.max(1),
    full_cell_height: full_cell_height.max(1),
    y_cell_start,
  })
}

fn hit_for_display(
  display: PageDisplay,
  column: u16,
  row: u16,
  clamp_to_display: bool,
) -> SelectionHit {
  let max_column = display
    .area
    .x
    .saturating_add(display.area.width.saturating_sub(1));
  let max_row = display
    .area
    .y
    .saturating_add(display.area.height.saturating_sub(1));
  let column = if clamp_to_display {
    column.clamp(display.area.x, max_column)
  } else {
    column
  };
  let row = if clamp_to_display {
    row.clamp(display.area.y, max_row)
  } else {
    row
  };
  let local_x = column.saturating_sub(display.area.x);
  let local_y = row.saturating_sub(display.area.y);
  let full_y = display.y_cell_start.saturating_add(local_y);
  let x0 = f64::from(local_x) / f64::from(display.full_cell_width.max(1)) * display.page_width;
  let x1 = f64::from(local_x.saturating_add(1)) / f64::from(display.full_cell_width.max(1))
    * display.page_width;
  let y0 = f64::from(full_y) / f64::from(display.full_cell_height.max(1)) * display.page_height;
  let y1 = f64::from(full_y.saturating_add(1)) / f64::from(display.full_cell_height.max(1))
    * display.page_height;
  let raw_cell_rect = PdfRect {
    x_min: x0,
    y_min: y0,
    x_max: x1,
    y_max: y1,
  }
  .normalized();
  let point = clamp_point_to_page(
    PdfPoint {
      x: (raw_cell_rect.x_min + raw_cell_rect.x_max) / 2.0,
      y: (raw_cell_rect.y_min + raw_cell_rect.y_max) / 2.0,
    },
    display.page_width,
    display.page_height,
  );
  let marker = marker_rect_around_point(
    point,
    raw_cell_rect.width().max(f64::EPSILON),
    raw_cell_rect.height().max(f64::EPSILON),
  );
  SelectionHit {
    page_index: display.page_index,
    page_width: display.page_width,
    page_height: display.page_height,
    point,
    cell_rect: marker,
  }
}

fn projected_point_for_display(display: PageDisplay, column: u16, row: u16) -> PdfPoint {
  let local_x = i32::from(column) - i32::from(display.area.x);
  let local_y = i32::from(row) - i32::from(display.area.y);
  let full_y = i32::from(display.y_cell_start) + local_y;
  PdfPoint {
    x: (f64::from(local_x) + 0.5) / f64::from(display.full_cell_width.max(1)) * display.page_width,
    y: (f64::from(full_y) + 0.5) / f64::from(display.full_cell_height.max(1)) * display.page_height,
  }
}

fn hit_for_selection_display(
  display: &SelectionDisplay,
  column: u16,
  row: u16,
  clamp_to_display: bool,
) -> SelectionHit {
  let max_column = display
    .area
    .x
    .saturating_add(display.area.width.saturating_sub(1));
  let max_row = display
    .area
    .y
    .saturating_add(display.area.height.saturating_sub(1));
  let column = if clamp_to_display {
    column.clamp(display.area.x, max_column)
  } else {
    column
  };
  let row = if clamp_to_display {
    row.clamp(display.area.y, max_row)
  } else {
    row
  };
  let local_x = column.saturating_sub(display.area.x);
  let local_y = row.saturating_sub(display.area.y);
  let area_width = f64::from(display.area.width.max(1));
  let area_height = f64::from(display.area.height.max(1));
  let rect_width = display.rect.width().max(1.0);
  let rect_height = display.rect.height().max(1.0);
  let x0 = display.rect.x_min + f64::from(local_x) / area_width * rect_width;
  let x1 = display.rect.x_min + f64::from(local_x.saturating_add(1)) / area_width * rect_width;
  let y0 = display.rect.y_min + f64::from(local_y) / area_height * rect_height;
  let y1 = display.rect.y_min + f64::from(local_y.saturating_add(1)) / area_height * rect_height;
  let raw_cell_rect = PdfRect {
    x_min: x0,
    y_min: y0,
    x_max: x1,
    y_max: y1,
  }
  .normalized();
  let point = clamp_point_to_page(
    PdfPoint {
      x: (raw_cell_rect.x_min + raw_cell_rect.x_max) / 2.0,
      y: (raw_cell_rect.y_min + raw_cell_rect.y_max) / 2.0,
    },
    display.page_width,
    display.page_height,
  );
  let marker = marker_rect_around_point(
    point,
    raw_cell_rect.width().max(f64::EPSILON),
    raw_cell_rect.height().max(f64::EPSILON),
  );
  SelectionHit {
    page_index: display.page_index,
    page_width: display.page_width,
    page_height: display.page_height,
    point,
    cell_rect: marker,
  }
}

fn projected_point_for_selection_display(
  display: &SelectionDisplay,
  column: u16,
  row: u16,
) -> PdfPoint {
  let local_x = i32::from(column) - i32::from(display.area.x);
  let local_y = i32::from(row) - i32::from(display.area.y);
  let rect_width = display.rect.width().max(1.0);
  let rect_height = display.rect.height().max(1.0);
  PdfPoint {
    x: display.rect.x_min
      + (f64::from(local_x) + 0.5) / f64::from(display.area.width.max(1)) * rect_width,
    y: display.rect.y_min
      + (f64::from(local_y) + 0.5) / f64::from(display.area.height.max(1)) * rect_height,
  }
}

fn anchor_from_hit(hit: SelectionHit) -> SelectionAnchor {
  SelectionAnchor {
    page_index: hit.page_index,
    page_width: hit.page_width,
    page_height: hit.page_height,
    point: hit.point,
    marker: hit.cell_rect,
  }
}

fn anchor_at_point(
  page_index: usize,
  page_width: f64,
  page_height: f64,
  point: PdfPoint,
  marker_width: f64,
  marker_height: f64,
  bounds: Option<PdfRect>,
) -> SelectionAnchor {
  let point = clamp_point_to_page(point, page_width, page_height);
  let point = if let Some(bounds) = bounds {
    PdfPoint {
      x: point.x.clamp(bounds.x_min, bounds.x_max),
      y: point.y.clamp(bounds.y_min, bounds.y_max),
    }
  } else {
    point
  };
  let marker = marker_rect_around_point(point, marker_width.max(1.0), marker_height.max(1.0));
  SelectionAnchor {
    page_index,
    page_width,
    page_height,
    point,
    marker,
  }
}

fn marker_rect_around_point(point: PdfPoint, marker_width: f64, marker_height: f64) -> PdfRect {
  PdfRect {
    x_min: point.x - marker_width / 2.0,
    y_min: point.y - marker_height / 2.0,
    x_max: point.x + marker_width / 2.0,
    y_max: point.y + marker_height / 2.0,
  }
  .normalized()
}

fn clamp_point_to_page(point: PdfPoint, page_width: f64, page_height: f64) -> PdfPoint {
  PdfPoint {
    x: point.x.clamp(0.0, page_width.max(1.0)),
    y: point.y.clamp(0.0, page_height.max(1.0)),
  }
}

fn bounded_opposite_point(
  reference: PdfPoint,
  point: PdfPoint,
  anchor: SelectionAnchor,
  bounds: Option<PdfSelection>,
) -> Option<PdfPoint> {
  let bounds = bounds.map(|selection| selection.rect).unwrap_or(PdfRect {
    x_min: 0.0,
    y_min: 0.0,
    x_max: anchor.page_width.max(1.0),
    y_max: anchor.page_height.max(1.0),
  });
  let rect = PdfRect {
    x_min: reference.x.min(point.x),
    y_min: reference.y.min(point.y),
    x_max: reference.x.max(point.x),
    y_max: reference.y.max(point.y),
  };
  let intersected = rect.intersection(bounds)?;
  Some(PdfPoint {
    x: if point.x >= reference.x {
      intersected.x_max
    } else {
      intersected.x_min
    },
    y: if point.y >= reference.y {
      intersected.y_max
    } else {
      intersected.y_min
    },
  })
}

fn normalized_distance(a: PdfPoint, b: PdfPoint, page_width: f64, page_height: f64) -> f64 {
  let dx = (a.x - b.x) / page_width.max(1.0);
  let dy = (a.y - b.y) / page_height.max(1.0);
  dx * dx + dy * dy
}

fn contains(area: Rect, column: u16, row: u16) -> bool {
  column >= area.x
    && column < area.x.saturating_add(area.width)
    && row >= area.y
    && row < area.y.saturating_add(area.height)
}

fn distance_to_rect(area: Rect, column: u16, row: u16) -> u32 {
  let x0 = i32::from(area.x);
  let y0 = i32::from(area.y);
  let x1 = i32::from(area.x.saturating_add(area.width.saturating_sub(1)));
  let y1 = i32::from(area.y.saturating_add(area.height.saturating_sub(1)));
  let x = i32::from(column).clamp(x0, x1);
  let y = i32::from(row).clamp(y0, y1);
  column
    .abs_diff(u16::try_from(x).unwrap_or_default())
    .pow(2)
    .saturating_add(row.abs_diff(u16::try_from(y).unwrap_or_default()).pow(2))
    .into()
}

fn fitted_page_area(
  area: Rect,
  cell_pixels: Option<(u16, u16)>,
  page_dimensions: Option<(u32, u32)>,
) -> Rect {
  if area.width == 0 || area.height == 0 {
    return area;
  }
  let (target_width, target_height) =
    page_target_pixels(area.width, area.height, cell_pixels, page_dimensions);
  let (cell_width, cell_height) = cell_pixels.unwrap_or((8, 16));
  let width = ceil_div_u32(target_width.max(1), u32::from(cell_width.max(1)))
    .min(u32::from(area.width))
    .max(1) as u16;
  let height = ceil_div_u32(target_height.max(1), u32::from(cell_height.max(1)))
    .min(u32::from(area.height))
    .max(1) as u16;
  Rect::new(
    area.x.saturating_add(area.width.saturating_sub(width) / 2),
    area
      .y
      .saturating_add(area.height.saturating_sub(height) / 2),
    width,
    height,
  )
}

fn page_target_pixels(
  width: u16,
  height: u16,
  cell_pixels: Option<(u16, u16)>,
  page_dimensions: Option<(u32, u32)>,
) -> (u32, u32) {
  let (cell_width, cell_height) = cell_pixels.unwrap_or((8, 16));
  let max_width = u32::from(width.max(1)).saturating_mul(u32::from(cell_width.max(1)));
  let max_height = u32::from(height.max(1)).saturating_mul(u32::from(cell_height.max(1)));
  let Some((page_width, page_height)) = page_dimensions else {
    return (max_width.max(1), max_height.max(1));
  };
  let scale = (f64::from(max_width.max(1)) / f64::from(page_width.max(1)))
    .min(f64::from(max_height.max(1)) / f64::from(page_height.max(1)));
  (
    (f64::from(page_width.max(1)) * scale)
      .round()
      .clamp(1.0, f64::from(u32::MAX)) as u32,
    (f64::from(page_height.max(1)) * scale)
      .round()
      .clamp(1.0, f64::from(u32::MAX)) as u32,
  )
}

fn ceil_div_u32(value: u32, divisor: u32) -> u32 {
  value
    .saturating_add(divisor.saturating_sub(1))
    .saturating_div(divisor.max(1))
}

fn safe_inner(area: Rect, horizontal: u16, vertical: u16) -> Rect {
  if area.width <= horizontal.saturating_mul(2) || area.height <= vertical.saturating_mul(2) {
    return Rect::new(area.x, area.y, 0, 0);
  }
  area.inner(Margin {
    horizontal,
    vertical,
  })
}
