//! The search view: query prompt, index, result list, and the delay that
//! keeps preview preloading from chasing every keystroke.

use framework_tui::{CommandState, Prompt};
use tokio::{sync::mpsc, time::sleep};

use crate::{
  event::AsyncEvent,
  search::{PdfSearchIndex, PdfSearchMatch},
};

use super::{App, ViewMode};

pub struct SearchState {
  pub prompt: Prompt,
  pub(super) command_state: CommandState,
  pub index: Option<PdfSearchIndex>,
  pub index_error: Option<String>,
  pub index_loading: bool,
  pub results: Vec<PdfSearchMatch>,
  pub selected: Option<usize>,
  pub scroll: u16,
  /// The opened result, highlighted in the viewer until the next viewer
  /// navigation.
  pub viewer_highlight: Option<PdfSearchMatch>,
  pub left_ratio: u16,
  pub right_ratio: u16,
  /// Preview preloading waits until typing pauses: each edit bumps
  /// `preload_generation`, and preloads run once `preload_ready_generation`
  /// catches up.
  preload_generation: u64,
  preload_ready_generation: u64,
  /// Queued preloads became stale after an edit and should be dropped.
  preload_reset_pending: bool,
}

impl SearchState {
  pub(super) fn new(left_ratio: u16, right_ratio: u16) -> Self {
    Self {
      prompt: Prompt::text("search: ", ""),
      command_state: CommandState::default(),
      index: None,
      index_error: None,
      index_loading: false,
      results: Vec::new(),
      selected: None,
      scroll: 0,
      viewer_highlight: None,
      left_ratio: left_ratio.max(1),
      right_ratio: right_ratio.max(1),
      preload_generation: 0,
      preload_ready_generation: 0,
      preload_reset_pending: false,
    }
  }

  /// Forgets the index and results of a document that was reloaded; the
  /// typed query stays.
  pub(super) fn reset_for_reload(&mut self) {
    self.index = None;
    self.index_error = None;
    self.index_loading = false;
    self.results.clear();
    self.selected = None;
    self.viewer_highlight = None;
    self.scroll = 0;
  }

  pub fn query(&self) -> &str {
    self.prompt.buffer().input.trim()
  }

  pub fn selected_match(&self) -> Option<&PdfSearchMatch> {
    self.selected.and_then(|index| self.results.get(index))
  }

  /// Result shown on row `row_offset` of the result list.
  pub(super) fn result_at_row(&self, row_offset: usize) -> Option<usize> {
    let index = usize::from(self.scroll) + row_offset;
    (index < self.results.len()).then_some(index)
  }

  /// Reruns the query; returns whether the selection or result count
  /// changed.
  pub(super) fn refresh_results(&mut self) -> bool {
    let previous = (self.selected, self.results.len());
    self.results = self
      .index
      .as_ref()
      .map(|index| index.search(self.prompt.buffer().input.trim()))
      .unwrap_or_default();
    if self.results.is_empty() {
      self.selected = None;
      self.scroll = 0;
    } else {
      self.selected = Some(self.selected.unwrap_or(0).min(self.results.len() - 1));
    }
    previous != (self.selected, self.results.len())
  }

  /// Scrolls so the selected result is within `visible_height` rows.
  pub fn clamp_scroll(&mut self, visible_height: u16) {
    let visible_height = visible_height.max(1);
    let max_scroll = u16::try_from(self.results.len())
      .unwrap_or(u16::MAX)
      .saturating_sub(visible_height);
    if let Some(selected) = self.selected {
      let selected = u16::try_from(selected).unwrap_or(u16::MAX);
      if selected < self.scroll {
        self.scroll = selected;
      } else if selected > self.scroll.saturating_add(visible_height - 1) {
        self.scroll = selected.saturating_sub(visible_height - 1);
      }
    }
    self.scroll = self.scroll.min(max_scroll);
  }

  /// Moves the selection; returns whether it changed.
  fn select_delta(&mut self, delta: isize) -> bool {
    if self.results.is_empty() {
      self.selected = None;
      return false;
    }
    let current = self.selected.unwrap_or(0);
    let next = current
      .saturating_add_signed(delta)
      .min(self.results.len() - 1);
    self.selected = Some(next);
    next != current
  }

  pub(super) fn make_preload_ready_now(&mut self) {
    self.preload_ready_generation = self.preload_generation;
  }
}

impl App {
  pub(super) fn enter_search_view(&mut self, tx: &mpsc::UnboundedSender<AsyncEvent>) {
    self.view = ViewMode::Search;
    self.key_dispatcher.clear();
    self.request_search_index(tx);
    self.refresh_search_results();
    self.search.make_preload_ready_now();
    self.lock_frame_navigation_if_enabled();
    self.set_message("search");
  }

  pub fn finish_search_index(&mut self, result: Result<PdfSearchIndex, String>) {
    self.search.index_loading = false;
    match result {
      Ok(index) => {
        self.search.index = Some(index);
        self.search.index_error = None;
        self.refresh_search_results();
        self.set_message(format!(
          "search index ready: {} result(s)",
          self.search.results.len()
        ));
      }
      Err(error) => {
        self.search.index = None;
        self.search.index_error = Some(error.clone());
        self.search.results.clear();
        self.search.selected = None;
        self.set_message(format!("search index failed: {error}"));
      }
    }
    self.search.make_preload_ready_now();
  }

  pub fn viewer_search_highlight_for(&self, page_index: usize) -> Option<&PdfSearchMatch> {
    self
      .search
      .viewer_highlight
      .as_ref()
      .filter(|result| result.page_index == page_index)
  }

  pub(super) fn refresh_search_results(&mut self) {
    if self.search.refresh_results() && self.view == ViewMode::Search {
      self.lock_frame_navigation_if_enabled();
    }
  }

  /// Rows moved by the search page up/down actions.
  pub(super) fn search_page_step(&self) -> isize {
    self.viewport_height.saturating_sub(3).max(1) as isize
  }

  pub(super) fn select_search_delta(&mut self, delta: isize) {
    if self.search.select_delta(delta) {
      self.search.make_preload_ready_now();
    }
  }

  pub(super) fn search_open(&mut self) {
    let Some(result) = self.search.selected_match().cloned() else {
      return;
    };
    let page_number = result.page_index + 1;
    self.jump_to_search_match(&result);
    self.search.viewer_highlight = Some(result);
    self.view = ViewMode::Viewer;
    self.key_dispatcher.clear();
    self.set_message(format!("jumped to search result on page {page_number}"));
  }

  pub fn search_preload_ready(&self) -> bool {
    self.view == ViewMode::Search
      && self.search.preload_ready_generation == self.search.preload_generation
  }

  /// The typing pause of `generation` elapsed; returns whether preloading
  /// may start now.
  pub fn finish_search_preload_delay(&mut self, generation: u64) -> bool {
    if self.view != ViewMode::Search || generation != self.search.preload_generation {
      return false;
    }
    self.search.preload_ready_generation = generation;
    true
  }

  pub fn take_search_preload_reset(&mut self) -> bool {
    std::mem::take(&mut self.search.preload_reset_pending)
  }

  /// Called after the query changed: preview preloading resumes once no
  /// further edit arrived for `search_preload_idle_ms`.
  pub(super) fn defer_search_preload_after_input(
    &mut self,
    tx: &mpsc::UnboundedSender<AsyncEvent>,
  ) {
    self.search.preload_generation = self.search.preload_generation.wrapping_add(1);
    self.search.preload_reset_pending = true;
    let generation = self.search.preload_generation;
    let delay =
      std::time::Duration::from_millis(self.settings.config.render.search_preload_idle_ms);
    let tx = tx.clone();
    tokio::spawn(async move {
      sleep(delay).await;
      let _ = tx.send(AsyncEvent::SearchPreloadReady { generation });
    });
  }
}
