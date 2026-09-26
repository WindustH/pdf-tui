use ratatui::layout::Rect;

use crate::{
  config::EffectiveLayoutConfig,
  layout::{ScrollLayout, compute_scroll_layout},
};

use super::App;

/// Everything a scroll layout depends on besides the document's page sizes
/// (a document reload drops the cache explicitly).
#[derive(Debug, Clone, PartialEq)]
struct ScrollLayoutKey {
  width: u16,
  height: u16,
  config: EffectiveLayoutConfig,
  cell_pixels: Option<(u16, u16)>,
}

/// The scroll layout for the current viewport, recomputed only when its
/// inputs change: computing it searches every candidate page width, which is
/// far too expensive to repeat on every frame.
#[derive(Debug, Clone)]
pub(super) struct CachedScrollLayout {
  key: ScrollLayoutKey,
  layout: ScrollLayout,
}

impl CachedScrollLayout {
  pub(super) fn layout(&self) -> &ScrollLayout {
    &self.layout
  }

  pub(super) fn viewport_height(&self) -> u16 {
    self.key.height
  }

  pub(super) fn scroll_divisor(&self) -> u16 {
    self.key.config.scroll_divisor
  }

  pub(super) fn max_scroll(&self) -> usize {
    self
      .layout
      .max_scroll_row(self.key.height, self.key.config.scroll_divisor)
  }
}

impl App {
  pub fn scroll_layout(&self) -> Option<&ScrollLayout> {
    self.scroll_layout.as_ref().map(CachedScrollLayout::layout)
  }

  /// Makes `scroll_layout()` match `viewport`. When the geometry changes
  /// under the reader (terminal resize, new cell size), the reading
  /// progress is carried over: scroll rows of the old layout mean nothing
  /// in the new one.
  pub fn prepare_scroll_layout(&mut self, viewport: Rect) {
    self.update_viewport(viewport);
    let key = ScrollLayoutKey {
      width: viewport.width,
      height: viewport.height,
      config: self.layout.clone(),
      cell_pixels: self.terminal_cell_pixels,
    };
    if self
      .scroll_layout
      .as_ref()
      .is_some_and(|cached| cached.key == key)
    {
      return;
    }
    let carried = if self.pending_progress.is_none() {
      self.current_progress()
    } else {
      None
    };
    let layout = compute_scroll_layout(
      self.document.page_count,
      key.width,
      key.height,
      &self.layout,
      |index| self.page_dimensions(index),
      self.terminal_cell_pixels,
    );
    self.scroll_layout = Some(CachedScrollLayout { key, layout });
    if carried.is_some() {
      self.pending_progress = carried;
    }
    self.scroll = self.scroll.min(self.max_scroll());
    self.apply_pending_progress_if_ready();
  }

  pub(super) fn scroll_down(&mut self) {
    if self.layout.is_scroll() {
      self.scroll_by_rows(1);
    } else {
      self.shift_grid_window(self.grid_row_step());
    }
  }

  pub(super) fn scroll_up(&mut self) {
    if self.layout.is_scroll() {
      self.scroll_by_rows(-1);
    } else {
      self.shift_grid_window(-self.grid_row_step());
    }
  }

  pub(super) fn page_down(&mut self) {
    if self.layout.is_scroll() {
      self.scroll_by_rows(i32::from(self.layout.scroll_divisor.max(1)));
    } else {
      self.shift_grid_window(self.grid_capacity_step());
    }
  }

  pub(super) fn page_up(&mut self) {
    if self.layout.is_scroll() {
      self.scroll_by_rows(-i32::from(self.layout.scroll_divisor.max(1)));
    } else {
      self.shift_grid_window(-self.grid_capacity_step());
    }
  }

  pub(super) fn next_page(&mut self) {
    self.focus_relative(1);
  }

  pub(super) fn previous_page(&mut self) {
    self.focus_relative(-1);
  }

  pub(super) fn home(&mut self) {
    self.focused_page = 0;
    self.scroll = 0;
    self.grid_start_page = 0;
  }

  pub(super) fn end(&mut self) {
    if self.document.page_count == 0 {
      self.focused_page = 0;
      self.scroll = 0;
      return;
    }
    self.focused_page = self.document.page_count - 1;
    if self.layout.is_scroll() {
      self.scroll_to_focused_page();
    } else {
      self.grid_start_page = self.grid_max_start(self.layout.grid_capacity());
    }
  }

  /// Shows `page_index` from its top: in scroll layouts the page's first
  /// slice becomes the top row, like `next_page` does.
  pub(super) fn jump_to_page(&mut self, page_index: usize) {
    if self.document.page_count == 0 {
      return;
    }
    let page_index = page_index.min(self.document.page_count - 1);
    if self.layout.is_scroll() && self.scroll_layout.is_some() {
      self.pending_progress = None;
      self.focused_page = page_index;
      self.scroll_to_focused_page();
    } else {
      self.set_progress_target(page_index as f64);
    }
  }

  fn focus_relative(&mut self, delta: isize) {
    if self.document.page_count == 0 {
      self.focused_page = 0;
      self.grid_start_page = 0;
      return;
    }
    if !self.layout.is_scroll() {
      self.shift_grid_window(delta);
      return;
    }
    self.focused_page = self
      .focused_page
      .saturating_add_signed(delta)
      .min(self.document.page_count - 1);
    self.scroll_to_focused_page();
  }

  fn grid_row_step(&self) -> isize {
    isize::try_from(self.layout.columns.max(1)).unwrap_or(isize::MAX)
  }

  fn grid_capacity_step(&self) -> isize {
    isize::try_from(self.layout.grid_capacity().max(1)).unwrap_or(isize::MAX)
  }

  fn shift_grid_window(&mut self, delta_pages: isize) {
    if self.document.page_count == 0 {
      self.focused_page = 0;
      self.grid_start_page = 0;
      return;
    }
    let capacity = self.layout.grid_capacity().max(1);
    self.clamp_grid_start(capacity);
    let max_start = self.grid_max_start(capacity);
    self.grid_start_page = self
      .grid_start_page
      .saturating_add_signed(delta_pages)
      .min(max_start);
    self.focused_page = self.grid_start_page.min(self.document.page_count - 1);
  }

  pub(super) fn clamp_grid_start(&mut self, capacity: usize) {
    self.grid_start_page = self.grid_start_page.min(self.grid_max_start(capacity));
  }

  fn grid_max_start(&self, capacity: usize) -> usize {
    self.document.page_count.saturating_sub(capacity.max(1))
  }

  fn scroll_by_rows(&mut self, delta: i32) {
    let max_scroll = self.max_scroll();
    self.scroll = self.scroll.saturating_add_signed(delta).min(max_scroll);
    self.update_focus_from_scroll();
  }

  fn scroll_to_focused_page(&mut self) {
    let Some(row_index) = self
      .scroll_layout()
      .and_then(|layout| layout.first_row_of_page(self.focused_page))
    else {
      return;
    };
    self.scroll = (row_index as u32).min(self.max_scroll());
  }

  pub(super) fn update_focus_from_scroll(&mut self) {
    if let Some(page_index) = self.scroll_layout().and_then(|layout| {
      layout
        .row_items(self.scroll as usize)
        .map(|item| item.page_index)
        .min()
    }) {
      self.focused_page = page_index;
    }
  }

  pub(super) fn max_scroll(&self) -> u32 {
    self
      .scroll_layout
      .as_ref()
      .map_or(0, |cached| cached.max_scroll() as u32)
  }
}
