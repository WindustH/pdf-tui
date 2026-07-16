use img_tui::TerminalCapability;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderConfig {
  pub pdfinfo_bin: String,
  pub pdftoppm_bin: String,
  pub pdftoppm_batch_pages: usize,
  pub pdftk_bin: String,
  pub pdftotext_bin: String,
  pub page_dpi: u16,
  pub chafa_bin: String,
  pub auto_detect: bool,
  pub chafa_args: Vec<String>,
  pub cache_max_bytes: u64,
  pub cache_compression_level: i32,
  pub cache_compression_threads: u32,
  pub memory_compression: bool,
  pub raw_memory_cache_max_bytes: u64,
  pub compressed_memory_cache_max_bytes: u64,
  pub prepared_memory_cache_max_bytes: u64,
  pub search_highlight_cache_max_bytes: u64,
  pub max_concurrent: usize,
  pub chafa_threads: usize,
  pub preload_ahead: usize,
  pub preload_behind: usize,
  pub preload_slice_ahead: usize,
  pub preload_slice_behind: usize,
  pub preload_terminal_ahead: usize,
  pub preload_terminal_behind: usize,
  pub passthrough: Option<String>,
  pub zellij_sixel: String,
}

impl Default for RenderConfig {
  fn default() -> Self {
    Self {
      pdfinfo_bin: "pdfinfo".to_string(),
      pdftoppm_bin: "pdftoppm".to_string(),
      pdftoppm_batch_pages: 4,
      pdftk_bin: "pdftk".to_string(),
      pdftotext_bin: "pdftotext".to_string(),
      page_dpi: 180,
      chafa_bin: "chafa".to_string(),
      auto_detect: true,
      chafa_args: vec![
        "--format=symbols".to_string(),
        "--colors=full".to_string(),
        "--symbols=block".to_string(),
        "--animate=off".to_string(),
        "--polite=on".to_string(),
      ],
      cache_max_bytes: 512 * 1024 * 1024,
      cache_compression_level: 3,
      cache_compression_threads: 2,
      memory_compression: true,
      raw_memory_cache_max_bytes: 32 * 1024 * 1024,
      compressed_memory_cache_max_bytes: 128 * 1024 * 1024,
      prepared_memory_cache_max_bytes: 128 * 1024 * 1024,
      search_highlight_cache_max_bytes: 64 * 1024 * 1024,
      max_concurrent: 4,
      chafa_threads: 1,
      preload_ahead: 4,
      preload_behind: 2,
      preload_slice_ahead: 3,
      preload_slice_behind: 1,
      preload_terminal_ahead: 2,
      preload_terminal_behind: 1,
      passthrough: None,
      zellij_sixel: "off".to_string(),
    }
  }
}

impl RenderConfig {
  pub fn apply_terminal_capability(&mut self, capability: &TerminalCapability) {
    self.chafa_args.retain(|arg| {
      !arg.starts_with("--format=")
        && !arg.starts_with("--colors=")
        && !arg.starts_with("--symbols=")
        && !arg.starts_with("--passthrough=")
    });
    self
      .chafa_args
      .insert(0, capability.symbols_arg().to_string());
    self
      .chafa_args
      .insert(0, capability.colors_arg().to_string());
    self.passthrough = capability.passthrough().map(str::to_string);
  }

  pub(super) fn normalize_defaults(&mut self) {
    self.page_dpi = self.page_dpi.max(36);
    self.max_concurrent = self.max_concurrent.max(1);
    self.pdftoppm_batch_pages = self.pdftoppm_batch_pages.max(1);
    self.raw_memory_cache_max_bytes = self.raw_memory_cache_max_bytes.max(1);
    self.compressed_memory_cache_max_bytes = self.compressed_memory_cache_max_bytes.max(1);
    self.prepared_memory_cache_max_bytes = self.prepared_memory_cache_max_bytes.max(1);
    self.search_highlight_cache_max_bytes = self.search_highlight_cache_max_bytes.max(1);
  }
}
