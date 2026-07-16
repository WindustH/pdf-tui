use super::App;

impl App {
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
    let Some(layout) = &self.last_scroll_layout else {
      return;
    };
    let Some((row_index, _)) = layout.rows.iter().enumerate().find(|(_, row)| {
      row.items.iter().any(|item| {
        layout
          .items
          .get(*item)
          .is_some_and(|item| item.page_index == self.focused_page)
      })
    }) else {
      return;
    };
    self.scroll = (row_index as u32).min(self.max_scroll());
  }

  pub(super) fn update_focus_from_scroll(&mut self) {
    let Some(layout) = &self.last_scroll_layout else {
      return;
    };
    let Some(row) = layout.rows.get(self.scroll as usize) else {
      return;
    };
    if let Some(page_index) = row
      .items
      .iter()
      .filter_map(|item| layout.items.get(*item))
      .map(|item| item.page_index)
      .min()
    {
      self.focused_page = page_index;
    }
  }

  pub(super) fn max_scroll(&self) -> u32 {
    self
      .last_scroll_layout
      .as_ref()
      .map(|layout| {
        crate::layout::max_scroll_row_for_viewport(
          layout,
          self.viewport_height,
          self.layout.scroll_divisor,
        ) as u32
      })
      .unwrap_or(0)
  }
}
