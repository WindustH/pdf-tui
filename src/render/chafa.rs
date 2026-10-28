//! Application settings adapter for the shared Chafa fallback.
use crate::config::RenderConfig;
use img_tui::RenderMode;
use std::path::Path;
pub(super) async fn run_chafa(
  image_path: &Path,
  width: u16,
  height: u16,
  config: &RenderConfig,
  mode: RenderMode,
) -> Result<Vec<u8>, String> {
  img_tui::chafa::run(
    image_path,
    width,
    height,
    &img_tui::chafa::ChafaConfig {
      chafa_bin: &config.chafa_bin,
      chafa_args: &config.chafa_args,
      chafa_threads: config.chafa_threads,
    },
    mode,
  )
  .await
}
