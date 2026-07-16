use tokio::sync::mpsc;

use crate::{
  event::AsyncEvent,
  search::{PdfSearchIndex, PdfSearchMatch},
};

use super::{App, ViewMode};

impl App {
  pub(super) fn enter_search_view(&mut self, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    self.view = ViewMode::Search;
    self.key_dispatcher.clear();
    self.request_search_index(tx);
    self.refresh_search_results();
    self.set_message("search");
  }

  pub fn finish_search_index(&mut self, result: Result<PdfSearchIndex, String>) {
    self.search_index_loading = false;
    match result {
      Ok(index) => {
        self.search_index = Some(index);
        self.search_index_error = None;
        self.refresh_search_results();
        self.set_message(format!(
          "search index ready: {} result(s)",
          self.search_results.len()
        ));
      }
      Err(error) => {
        self.search_index = None;
        self.search_index_error = Some(error.clone());
        self.search_results.clear();
        self.search_selected = None;
        self.set_message(format!("search index failed: {error}"));
      }
    }
  }

  pub fn selected_search_match(&self) -> Option<&PdfSearchMatch> {
    self
      .search_selected
      .and_then(|index| self.search_results.get(index))
  }

  pub fn refresh_search_results(&mut self) {
    let query = self.search_prompt.buffer().input.trim();
    self.search_results = self
      .search_index
      .as_ref()
      .map(|index| index.search(query))
      .unwrap_or_default();
    if self.search_results.is_empty() {
      self.search_selected = None;
      self.search_scroll = 0;
    } else {
      self.search_selected = Some(
        self
          .search_selected
          .unwrap_or(0)
          .min(self.search_results.len().saturating_sub(1)),
      );
    }
  }

  pub fn clamp_search_scroll(&mut self, visible_height: u16) {
    let max_scroll = (self.search_results.len() as u16).saturating_sub(visible_height.max(1));
    if let Some(selected) = self.search_selected {
      let selected = selected as u16;
      if selected < self.search_scroll {
        self.search_scroll = selected;
      } else {
        let bottom = self
          .search_scroll
          .saturating_add(visible_height.saturating_sub(1));
        if selected > bottom {
          self.search_scroll = selected.saturating_sub(visible_height.saturating_sub(1));
        }
      }
    }
    self.search_scroll = self.search_scroll.min(max_scroll);
  }

  pub(super) fn search_next(&mut self) {
    self.select_search_delta(1);
  }

  pub(super) fn search_previous(&mut self) {
    self.select_search_delta(-1);
  }

  pub(super) fn search_page_down(&mut self) {
    let step = self.viewport_height.saturating_sub(3).max(1) as isize;
    self.select_search_delta(step);
  }

  pub(super) fn search_page_up(&mut self) {
    let step = self.viewport_height.saturating_sub(3).max(1) as isize;
    self.select_search_delta(-step);
  }

  pub(super) fn search_open(&mut self) {
    let Some(result) = self.selected_search_match() else {
      return;
    };
    let progress = result.page_index as f64 + result.rect.y_min / result.page_height.max(1.0);
    let page_number = result.page_index + 1;
    self.set_progress_target(progress);
    self.view = ViewMode::Viewer;
    self.key_dispatcher.clear();
    self.set_message(format!("jumped to search result on page {page_number}"));
  }

  fn select_search_delta(&mut self, delta: isize) {
    if self.search_results.is_empty() {
      self.search_selected = None;
      return;
    }
    let current = self.search_selected.unwrap_or(0);
    let next = current
      .saturating_add_signed(delta)
      .min(self.search_results.len().saturating_sub(1));
    self.search_selected = Some(next);
  }
}
