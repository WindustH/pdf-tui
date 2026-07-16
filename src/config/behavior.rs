use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BehaviorConfig {
  pub scroll_lines: u16,
}

impl Default for BehaviorConfig {
  fn default() -> Self {
    Self { scroll_lines: 4 }
  }
}

impl BehaviorConfig {
  pub(super) fn normalize_defaults(&mut self) {
    self.scroll_lines = self.scroll_lines.max(1);
  }
}
