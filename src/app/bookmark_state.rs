use crate::bookmarks;

use super::{App, EditorRequest, ViewMode};

impl App {
  pub fn visible_bookmark_indices(&self) -> Vec<usize> {
    let mut visible = Vec::new();
    let mut hidden_below = None;
    for (index, bookmark) in self.bookmarks.iter().enumerate() {
      if let Some(level) = hidden_below {
        if bookmark.level > level {
          continue;
        }
        hidden_below = None;
      }
      visible.push(index);
      if self.bookmark_has_children(index) && !self.bookmarks_expanded.contains(&index) {
        hidden_below = Some(bookmark.level);
      }
    }
    visible
  }

  pub fn bookmark_has_children(&self, index: usize) -> bool {
    let Some(bookmark) = self.bookmarks.get(index) else {
      return false;
    };
    self
      .bookmarks
      .get(index.saturating_add(1))
      .is_some_and(|next| next.level > bookmark.level)
  }

  pub fn selected_bookmark(&self) -> Option<&crate::bookmarks::PdfBookmark> {
    self
      .bookmarks_selected
      .and_then(|index| self.bookmarks.get(index))
  }

  pub fn clamp_bookmarks_scroll(&mut self, visible_height: u16) {
    let rows = self.visible_bookmark_indices();
    let max_scroll = (rows.len() as u16).saturating_sub(visible_height.max(1));
    if let Some(selected) = self.bookmarks_selected
      && let Some(position) = rows.iter().position(|index| *index == selected)
    {
      let position = position as u16;
      if position < self.bookmarks_scroll {
        self.bookmarks_scroll = position;
      } else {
        let bottom = self
          .bookmarks_scroll
          .saturating_add(visible_height.saturating_sub(1));
        if position > bottom {
          self.bookmarks_scroll = position.saturating_sub(visible_height.saturating_sub(1));
        }
      }
    }
    self.bookmarks_scroll = self.bookmarks_scroll.min(max_scroll);
  }

  pub(super) fn enter_bookmarks_view(&mut self) {
    self.view = ViewMode::Bookmarks;
    self.key_dispatcher.clear();
    self.select_bookmark_near_current_progress();
    self.lock_frame_navigation_if_enabled();
    if let Some(error) = &self.bookmarks_error {
      self.set_message(format!("bookmarks unavailable: {error}"));
    } else if self.bookmarks.is_empty() {
      self.set_message("no bookmarks");
    } else {
      self.set_message("bookmarks");
    }
  }

  pub(super) fn start_bookmarks_edit(&mut self) {
    if let Some(error) = &self.bookmarks_error {
      self.set_message(format!("bookmarks unavailable: {error}"));
      return;
    }
    let draft = bookmarks::bookmarks_edit_draft(
      &self.document.path,
      &self.bookmarks,
      self.document.page_count,
    );
    self.set_editor_request(EditorRequest::Bookmarks {
      original: self.bookmarks.clone(),
      draft,
    });
    self.set_message("editing bookmarks");
  }

  pub(super) fn bookmarks_next(&mut self) {
    self.select_visible_bookmark_delta(1);
  }

  pub(super) fn bookmarks_previous(&mut self) {
    self.select_visible_bookmark_delta(-1);
  }

  pub(super) fn bookmarks_page_down(&mut self) {
    let step = self.viewport_height.saturating_sub(2).max(1) as isize;
    self.select_visible_bookmark_delta(step);
  }

  pub(super) fn bookmarks_page_up(&mut self) {
    let step = self.viewport_height.saturating_sub(2).max(1) as isize;
    self.select_visible_bookmark_delta(-step);
  }

  pub(super) fn bookmarks_toggle(&mut self) {
    let Some(index) = self.bookmarks_selected else {
      return;
    };
    if !self.bookmark_has_children(index) {
      return;
    }
    if self.bookmarks_expanded.remove(&index) {
      self.bookmarks_all_expanded = false;
    } else {
      self.bookmarks_expanded.insert(index);
      self.update_bookmarks_all_expanded();
    }
  }

  pub(super) fn bookmarks_toggle_all(&mut self) {
    if self.bookmarks_all_expanded {
      self.bookmarks_expanded.clear();
      self.bookmarks_all_expanded = false;
      self.ensure_selected_bookmark_visible_after_collapse();
      return;
    }
    self.bookmarks_expanded.clear();
    for index in 0..self.bookmarks.len() {
      if self.bookmark_has_children(index) {
        self.bookmarks_expanded.insert(index);
      }
    }
    self.bookmarks_all_expanded = true;
  }

  pub(super) fn bookmarks_open(&mut self) {
    let Some(bookmark) = self.selected_bookmark() else {
      return;
    };
    let page_index = bookmark
      .page_index
      .min(self.document.page_count.saturating_sub(1));
    let title = bookmark.title.clone();
    self.set_progress_target(page_index as f64);
    self.view = ViewMode::Viewer;
    self.key_dispatcher.clear();
    self.set_message(format!("jumped to bookmark: {title}"));
  }

  pub(super) fn bookmarks_panel_narrower(&mut self) {
    self.bookmarks_left_ratio = self.bookmarks_left_ratio.saturating_sub(1).max(1);
  }

  pub(super) fn bookmarks_panel_wider(&mut self) {
    self.bookmarks_left_ratio = self.bookmarks_left_ratio.saturating_add(1).min(8);
  }

  fn select_bookmark_near_current_progress(&mut self) {
    if self.bookmarks.is_empty() {
      self.bookmarks_selected = None;
      return;
    }
    let progress = self
      .current_progress()
      .or(self.pending_progress)
      .unwrap_or(self.focused_page as f64);
    let mut best = 0;
    let mut best_distance = f64::INFINITY;
    for bookmark in &self.bookmarks {
      let distance = (bookmark.page_index as f64 - progress).abs();
      if distance < best_distance {
        best = bookmark.id;
        best_distance = distance;
      }
    }
    self.bookmarks_selected = Some(best);
    self.expand_bookmark_ancestors(best);
    self.bookmarks_scroll = 0;
  }

  fn select_visible_bookmark_delta(&mut self, delta: isize) {
    let visible = self.visible_bookmark_indices();
    if visible.is_empty() {
      self.bookmarks_selected = None;
      return;
    }
    let current = self
      .bookmarks_selected
      .and_then(|selected| visible.iter().position(|index| *index == selected))
      .unwrap_or(0);
    let next = current
      .saturating_add_signed(delta)
      .min(visible.len().saturating_sub(1));
    self.bookmarks_selected = visible.get(next).copied();
  }

  fn expand_bookmark_ancestors(&mut self, mut index: usize) {
    while let Some(parent) = self.bookmark_parent(index) {
      self.bookmarks_expanded.insert(parent);
      index = parent;
    }
  }

  fn bookmark_parent(&self, index: usize) -> Option<usize> {
    let level = self.bookmarks.get(index)?.level;
    if level <= 1 {
      return None;
    }
    (0..index)
      .rev()
      .find(|candidate| self.bookmarks[*candidate].level < level)
  }

  fn ensure_selected_bookmark_visible_after_collapse(&mut self) {
    let Some(mut selected) = self.bookmarks_selected else {
      return;
    };
    while let Some(parent) = self.bookmark_parent(selected) {
      selected = parent;
    }
    self.bookmarks_selected = Some(selected);
  }

  fn update_bookmarks_all_expanded(&mut self) {
    let expandable = self
      .bookmarks
      .iter()
      .enumerate()
      .filter(|(index, _)| self.bookmark_has_children(*index))
      .map(|(index, _)| index)
      .collect::<Vec<_>>();
    self.bookmarks_all_expanded = !expandable.is_empty()
      && expandable
        .iter()
        .all(|index| self.bookmarks_expanded.contains(index));
  }
}
