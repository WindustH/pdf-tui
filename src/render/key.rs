use img_tui::{NativeImageConfig, RenderMode};
use sha2::{Digest, Sha256};

use crate::{config::RenderConfig, pdf::PageImage};

use super::RenderKind;

pub(super) fn hash_render_kind(
  hasher: &mut Sha256,
  kind: RenderKind,
  _include_viewport_offset: bool,
) {
  match kind {
    RenderKind::Fit => hasher.update(b"fit"),
  }
}

pub(super) fn render_cache_key(
  page: &PageImage,
  width: u16,
  height: u16,
  kind: RenderKind,
  config: &RenderConfig,
  native_config: &NativeImageConfig,
  mode: RenderMode,
) -> String {
  let mut hasher = Sha256::new();
  hasher.update(b"pdf-tui-render-cache-key-v4");
  hasher.update(page.path.to_string_lossy().as_bytes());
  hasher.update(page.page_index.to_le_bytes());
  hasher.update(page.size_bytes.to_le_bytes());
  hasher.update(page.modified_nanos.to_le_bytes());
  hasher.update(width.to_le_bytes());
  hasher.update(height.to_le_bytes());
  hash_render_kind(&mut hasher, kind, true);
  hasher.update(mode.label().as_bytes());
  hasher.update([0]);
  hash_render_config(&mut hasher, config);
  hash_native_config(&mut hasher, native_config);
  for arg in &config.chafa_args {
    hasher.update(arg.as_bytes());
    hasher.update([0]);
  }
  hex::encode(hasher.finalize())
}

pub(super) fn hash_render_config(hasher: &mut Sha256, config: &RenderConfig) {
  hasher.update(b"render-v3");
  hasher.update(config.chafa_bin.as_bytes());
  hasher.update([0]);
  hasher.update(config.chafa_threads.to_le_bytes());
  if let Some(passthrough) = &config.passthrough {
    hasher.update(passthrough.as_bytes());
  }
  hasher.update([0]);
  for arg in &config.chafa_args {
    hasher.update(arg.as_bytes());
    hasher.update([0]);
  }
}

pub(super) fn hash_native_config(hasher: &mut Sha256, config: &NativeImageConfig) {
  hasher.update(config.cell_pixels.unwrap_or((0, 0)).0.to_le_bytes());
  hasher.update(config.cell_pixels.unwrap_or((0, 0)).1.to_le_bytes());
  hasher.update([0]);
  if let Some(passthrough) = &config.passthrough {
    hasher.update(passthrough.as_bytes());
  }
  hasher.update([0]);
  hasher.update([u8::from(config.kitty_unicode_placeholders)]);
  hasher.update([0]);
}

pub(super) fn kitty_image_id(
  page: &PageImage,
  width: u16,
  height: u16,
  mode: RenderMode,
) -> Option<u32> {
  if mode != RenderMode::Kitty {
    return None;
  }
  let mut hasher = Sha256::new();
  hasher.update(b"pdf-tui-kitty-image-v3");
  hasher.update(page.path.to_string_lossy().as_bytes());
  hasher.update(page.page_index.to_le_bytes());
  hasher.update(page.size_bytes.to_le_bytes());
  hasher.update(page.modified_nanos.to_le_bytes());
  hasher.update(width.to_le_bytes());
  hasher.update(height.to_le_bytes());
  hasher.update(mode.label().as_bytes());
  let digest = hasher.finalize();
  let image_id = u32::from_le_bytes(digest[..4].try_into().unwrap_or_default()) & 0x00ff_ffff;
  Some(image_id.max(1))
}

pub(super) fn kitty_placement_id(
  _page: &PageImage,
  mode: RenderMode,
  image_id: Option<u32>,
) -> Option<u32> {
  if mode == RenderMode::Kitty {
    image_id
  } else {
    None
  }
}

pub(super) fn render_fingerprint(bytes: &[u8]) -> u64 {
  let mut hasher = Sha256::new();
  hasher.update(bytes);
  let digest = hasher.finalize();
  u64::from_le_bytes(digest[..8].try_into().unwrap_or_default())
}
