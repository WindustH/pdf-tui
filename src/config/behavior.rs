use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BehaviorConfig {
  pub scroll_lines: u16,
  pub frame_sync_navigation_viewer: bool,
  pub frame_sync_navigation_bookmarks: bool,
  pub frame_sync_navigation_search: bool,
  pub auto_refresh: bool,
  pub auto_refresh_poll_ms: u64,
  pub auto_refresh_min_interval_ms: u64,
  pub bookmarks_left_ratio: u16,
  pub bookmarks_right_ratio: u16,
  pub search_left_ratio: u16,
  pub search_right_ratio: u16,
}

impl Default for BehaviorConfig {
  fn default() -> Self {
    Self {
      scroll_lines: 4,
      frame_sync_navigation_viewer: true,
      frame_sync_navigation_bookmarks: false,
      frame_sync_navigation_search: false,
      auto_refresh: false,
      auto_refresh_poll_ms: 500,
      auto_refresh_min_interval_ms: 1500,
      bookmarks_left_ratio: 2,
      bookmarks_right_ratio: 1,
      search_left_ratio: 2,
      search_right_ratio: 1,
    }
  }
}

impl BehaviorConfig {
  pub(super) fn normalize_defaults(&mut self) {
    self.scroll_lines = self.scroll_lines.max(1);
    self.auto_refresh_poll_ms = self.auto_refresh_poll_ms.max(200);
    self.auto_refresh_min_interval_ms = self.auto_refresh_min_interval_ms.max(500);
    self.bookmarks_left_ratio = self.bookmarks_left_ratio.max(1);
    self.bookmarks_right_ratio = self.bookmarks_right_ratio.max(1);
    self.search_left_ratio = self.search_left_ratio.max(1);
    self.search_right_ratio = self.search_right_ratio.max(1);
  }
}
