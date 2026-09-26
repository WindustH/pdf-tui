//! The bookmarks view: an outline tree with per-entry expansion and a
//! selected entry, and the app actions that act on it.

use std::collections::HashSet;

use crate::bookmarks::{self, PdfBookmark};

use super::{App, EditorRequest, ViewMode};

/// The document outline and the tree state of the bookmarks view.
#[derive(Debug, Default)]
pub struct BookmarkTree {
  pub entries: Vec<PdfBookmark>,
  /// Why the outline could not be read (e.g. `pdftk` is missing).
  pub error: Option<String>,
  pub expanded: HashSet<usize>,
  pub selected: Option<usize>,
  pub scroll: u16,
  all_expanded: bool,
  pub left_ratio: u16,
  pub right_ratio: u16,
}

impl BookmarkTree {
  pub(super) fn new(
    loaded: Result<Vec<PdfBookmark>, String>,
    left_ratio: u16,
    right_ratio: u16,
  ) -> Self {
    let mut tree = Self {
      left_ratio: left_ratio.max(1),
      right_ratio: right_ratio.max(1),
      ..Self::default()
    };
    tree.replace(loaded);
    tree
  }

  /// Installs a freshly read outline; expansion and selection start over.
  pub(super) fn replace(&mut self, loaded: Result<Vec<PdfBookmark>, String>) {
    (self.entries, self.error) = match loaded {
      Ok(entries) => (entries, None),
      Err(error) => (Vec::new(), Some(error)),
    };
    self.expanded.clear();
    self.selected = None;
    self.scroll = 0;
    self.all_expanded = false;
  }

  /// Entry indices shown in the tree: children of collapsed entries are
  /// hidden.
  pub fn visible_indices(&self) -> Vec<usize> {
    let mut visible = Vec::new();
    let mut hidden_below = None;
    for (index, bookmark) in self.entries.iter().enumerate() {
      if let Some(level) = hidden_below {
        if bookmark.level > level {
          continue;
        }
        hidden_below = None;
      }
      visible.push(index);
      if self.has_children(index) && !self.expanded.contains(&index) {
        hidden_below = Some(bookmark.level);
      }
    }
    visible
  }

  pub fn has_children(&self, index: usize) -> bool {
    let Some(bookmark) = self.entries.get(index) else {
      return false;
    };
    self
      .entries
      .get(index.saturating_add(1))
      .is_some_and(|next| next.level > bookmark.level)
  }

  pub fn selected_entry(&self) -> Option<&PdfBookmark> {
    self.selected.and_then(|index| self.entries.get(index))
  }

  /// Entry shown on row `row_offset` of the tree panel.
  pub(super) fn index_at_row(&self, row_offset: usize) -> Option<usize> {
    self
      .visible_indices()
      .get(usize::from(self.scroll) + row_offset)
      .copied()
  }

  /// Scrolls so the selected entry is within `visible_height` rows.
  pub fn clamp_scroll(&mut self, visible_height: u16) {
    let rows = self.visible_indices();
    let visible_height = visible_height.max(1);
    let max_scroll = u16::try_from(rows.len())
      .unwrap_or(u16::MAX)
      .saturating_sub(visible_height);
    if let Some(position) = self
      .selected
      .and_then(|selected| rows.iter().position(|index| *index == selected))
    {
      let position = u16::try_from(position).unwrap_or(u16::MAX);
      if position < self.scroll {
        self.scroll = position;
      } else if position > self.scroll.saturating_add(visible_height - 1) {
        self.scroll = position.saturating_sub(visible_height - 1);
      }
    }
    self.scroll = self.scroll.min(max_scroll);
  }

  pub(super) fn select_visible_delta(&mut self, delta: isize) {
    let visible = self.visible_indices();
    if visible.is_empty() {
      self.selected = None;
      return;
    }
    let current = self
      .selected
      .and_then(|selected| visible.iter().position(|index| *index == selected))
      .unwrap_or(0);
    let next = current.saturating_add_signed(delta).min(visible.len() - 1);
    self.selected = visible.get(next).copied();
  }

  /// Selects the entry whose page is closest to the 0-based reading
  /// `progress`, revealing it by expanding its ancestors.
  pub(super) fn select_nearest(&mut self, progress: f64) {
    let Some(best) = self
      .entries
      .iter()
      .enumerate()
      .min_by(|(_, left), (_, right)| {
        let distance = |bookmark: &PdfBookmark| (bookmark.page_index as f64 - progress).abs();
        distance(left).total_cmp(&distance(right))
      })
      .map(|(index, _)| index)
    else {
      self.selected = None;
      return;
    };
    self.selected = Some(best);
    let mut index = best;
    while let Some(parent) = self.parent(index) {
      self.expanded.insert(parent);
      index = parent;
    }
    self.scroll = 0;
  }

  pub(super) fn toggle_selected(&mut self) {
    let Some(index) = self.selected else {
      return;
    };
    if !self.has_children(index) {
      return;
    }
    if self.expanded.remove(&index) {
      self.all_expanded = false;
    } else {
      self.expanded.insert(index);
      self.all_expanded = (0..self.entries.len())
        .filter(|index| self.has_children(*index))
        .all(|index| self.expanded.contains(&index));
    }
  }

  /// Expands every entry, or collapses everything when all are expanded.
  pub(super) fn toggle_all(&mut self) {
    self.expanded.clear();
    if self.all_expanded {
      self.all_expanded = false;
      // Keep the selection on a visible entry: its topmost ancestor.
      if let Some(mut selected) = self.selected {
        while let Some(parent) = self.parent(selected) {
          selected = parent;
        }
        self.selected = Some(selected);
      }
      return;
    }
    self.expanded = (0..self.entries.len())
      .filter(|index| self.has_children(*index))
      .collect();
    self.all_expanded = true;
  }

  pub(super) fn narrow_panel(&mut self) {
    self.left_ratio = self.left_ratio.saturating_sub(1).max(1);
  }

  pub(super) fn widen_panel(&mut self) {
    self.left_ratio = self.left_ratio.saturating_add(1).min(8);
  }

  fn parent(&self, index: usize) -> Option<usize> {
    let level = self.entries.get(index)?.level;
    if level <= 1 {
      return None;
    }
    (0..index)
      .rev()
      .find(|candidate| self.entries[*candidate].level < level)
  }
}

impl App {
  pub(super) fn enter_bookmarks_view(&mut self) {
    self.view = ViewMode::Bookmarks;
    self.key_dispatcher.clear();
    let progress = self
      .current_progress()
      .or(self.pending_progress)
      .unwrap_or(self.focused_page as f64);
    self.bookmarks.select_nearest(progress);
    self.lock_frame_navigation_if_enabled();
    if let Some(error) = &self.bookmarks.error {
      self.set_message(format!("bookmarks unavailable: {error}"));
    } else if self.bookmarks.entries.is_empty() {
      self.set_message("no bookmarks");
    } else {
      self.set_message("bookmarks");
    }
  }

  pub(super) fn start_bookmarks_edit(&mut self) {
    if let Some(error) = &self.bookmarks.error {
      self.set_message(format!("bookmarks unavailable: {error}"));
      return;
    }
    let draft = bookmarks::bookmarks_edit_draft(
      &self.document.path,
      &self.bookmarks.entries,
      self.document.page_count,
    );
    self.set_editor_request(EditorRequest::Bookmarks {
      original: self.bookmarks.entries.clone(),
      draft,
    });
    self.set_message("editing bookmarks");
  }

  /// Rows moved by the bookmark page up/down actions.
  pub(super) fn bookmarks_page_step(&self) -> isize {
    self.viewport_height.saturating_sub(2).max(1) as isize
  }

  pub(super) fn bookmarks_open(&mut self) {
    let Some(bookmark) = self.bookmarks.selected_entry() else {
      return;
    };
    let page_index = bookmark
      .page_index
      .min(self.document.page_count.saturating_sub(1));
    let title = bookmark.title.clone();
    self.jump_to_page(page_index);
    self.view = ViewMode::Viewer;
    self.key_dispatcher.clear();
    self.set_message(format!("jumped to bookmark: {title}"));
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn tree(levels_and_pages: &[(u16, usize)]) -> BookmarkTree {
    let entries = levels_and_pages
      .iter()
      .enumerate()
      .map(|(id, (level, page_index))| PdfBookmark {
        id,
        title: format!("entry {id}"),
        level: *level,
        page_index: *page_index,
      })
      .collect();
    BookmarkTree::new(Ok(entries), 2, 1)
  }

  #[test]
  fn selecting_nearest_expands_ancestors_and_toggle_all_round_trips() {
    // 0: ch1 (p0) > 1: s1.1 (p2) > 2: s1.1.1 (p3); 3: ch2 (p9)
    let mut tree = tree(&[(1, 0), (2, 2), (3, 3), (1, 9)]);
    assert_eq!(tree.visible_indices(), vec![0, 3]);
    tree.select_nearest(3.2);
    assert_eq!(tree.selected, Some(2));
    assert_eq!(tree.visible_indices(), vec![0, 1, 2, 3]);

    tree.toggle_all();
    assert_eq!(tree.visible_indices(), vec![0, 1, 2, 3]);
    tree.toggle_all();
    // Collapsing everything moves the selection to the visible ancestor.
    assert_eq!(tree.visible_indices(), vec![0, 3]);
    assert_eq!(tree.selected, Some(0));

    tree.select_visible_delta(5);
    assert_eq!(tree.selected, Some(3));
    tree.clamp_scroll(1);
    assert_eq!(tree.scroll, 1);
    assert_eq!(tree.index_at_row(0), Some(3));
  }
}
