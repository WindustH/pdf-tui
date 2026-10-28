use crate::render::RenderedImage;
use img_tui::{ProtocolImage, ProtocolPlacement, RenderMode, cache::MemoryCache, native_image};
use std::{io::Cursor, sync::Arc};

#[derive(Debug)]
struct CompressedRenderedImage {
  mode: RenderMode,
  data: Vec<u8>,
  refresh: Option<Vec<u8>>,
  placement: Option<ProtocolPlacement>,
  fingerprint: u64,
  erase: Option<Arc<str>>,
}
#[derive(Debug)]
pub(super) struct RenderedImageMemoryCache {
  raw: MemoryCache<String, RenderedImage>,
  compressed: MemoryCache<String, CompressedRenderedImage>,
  current: Option<Arc<RenderedImage>>,
  compression: bool,
}
impl RenderedImageMemoryCache {
  pub(super) fn new(raw_max_bytes: usize, compressed_max_bytes: usize, compression: bool) -> Self {
    Self {
      raw: MemoryCache::new(raw_max_bytes as u64),
      compressed: MemoryCache::new(compressed_max_bytes as u64),
      current: None,
      compression,
    }
  }
  pub(super) fn clear(&mut self) {
    self.current = None;
    self.raw.clear();
    self.compressed.clear();
  }
  pub(super) fn contains_key(&self, key: &str) -> bool {
    self.raw.contains_key(key) || self.compressed.contains_key(key)
  }
  pub(super) fn get(&mut self, key: &str) -> Option<&RenderedImage> {
    self.current = self.raw.get(key);
    if self.current.is_none() {
      let compressed = self.compressed.get(key)?;
      match compressed.decompress() {
        Ok(image) => {
          let bytes = rendered_image_bytes(&image);
          self.raw.insert(key.to_string(), image, bytes as u64);
          self.current = self.raw.get(key);
        }
        Err(_) => {
          self.compressed.remove(key);
          return None;
        }
      }
    }
    self.current.as_deref()
  }
  pub(super) fn touch(&mut self, key: &str) {
    self.raw.touch(key);
    self.compressed.touch(key);
  }
  pub(super) fn insert(&mut self, key: String, image: RenderedImage) {
    if self.compression
      && let Some(compressed) = CompressedRenderedImage::compress(&image)
    {
      let bytes = compressed.stored_bytes();
      self
        .compressed
        .insert(key.clone(), compressed, bytes as u64);
    }
    let bytes = rendered_image_bytes(&image);
    self.raw.insert(key, image, bytes as u64);
  }
}
impl CompressedRenderedImage {
  fn compress(image: &RenderedImage) -> Option<Self> {
    let RenderedImage::Protocol(image) = image else {
      return None;
    };
    Some(Self {
      mode: image.mode,
      data: compress_memory_bytes(image.data.as_bytes()).ok()?,
      refresh: image
        .refresh
        .as_ref()
        .map(|refresh| compress_memory_bytes(refresh.as_bytes()))
        .transpose()
        .ok()?,
      placement: image.placement,
      fingerprint: image.fingerprint,
      erase: image.erase.clone(),
    })
  }

  fn decompress(&self) -> Result<RenderedImage, String> {
    Ok(RenderedImage::Protocol(ProtocolImage {
      mode: self.mode,
      data: decompress_memory_str(&self.data)?,
      refresh: self
        .refresh
        .as_ref()
        .map(|refresh| decompress_memory_str(refresh))
        .transpose()?,
      placement: self.placement,
      fingerprint: self.fingerprint,
      erase: self.erase.clone(),
    }))
  }

  fn stored_bytes(&self) -> usize {
    self
      .data
      .len()
      .saturating_add(self.refresh.as_ref().map_or(0, Vec::len))
      .saturating_add(self.erase.as_ref().map_or(0, |erase| erase.len()))
      .saturating_add(256)
  }
}

#[derive(Debug)]
pub(super) struct PreparedImageMemoryCache {
  entries: MemoryCache<String, native_image::PreparedNativeImage>,
}
impl PreparedImageMemoryCache {
  pub(super) fn new(max_bytes: usize) -> Self {
    Self {
      entries: MemoryCache::new(max_bytes as u64),
    }
  }
  pub(super) fn get(&mut self, key: &str) -> Option<native_image::PreparedNativeImage> {
    self.entries.get(key).map(|v| (*v).clone())
  }
  pub(super) fn insert(
    &mut self,
    key: String,
    image: native_image::PreparedNativeImage,
    bytes: usize,
  ) {
    self.entries.insert(key, image, bytes as u64);
  }
}
pub(super) fn memory_cache_bytes(configured: u64) -> usize {
  usize::try_from(configured).unwrap_or(usize::MAX)
}

fn rendered_image_bytes(image: &RenderedImage) -> usize {
  match image {
    RenderedImage::Symbols { text, .. } => text
      .lines
      .iter()
      .map(|line| {
        32 + line
          .spans
          .iter()
          .map(|span| 32 + span.content.len())
          .sum::<usize>()
      })
      .sum::<usize>(),
    RenderedImage::Protocol(image) => image.payload_len().saturating_add(256),
  }
}

fn compress_memory_bytes(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
  zstd::stream::encode_all(Cursor::new(bytes), 1)
}

fn decompress_memory_str(bytes: &[u8]) -> Result<Arc<str>, String> {
  let bytes = zstd::stream::decode_all(Cursor::new(bytes))
    .map_err(|error| format!("memory cache decompression failed: {error}"))?;
  String::from_utf8(bytes)
    .map(Arc::from)
    .map_err(|error| error.to_string())
}

pub(super) fn prepared_image_estimated_bytes(
  width_cells: u16,
  height_cells: u16,
  cell_pixels: Option<(u16, u16)>,
) -> usize {
  let (cell_width, cell_height) = cell_pixels.unwrap_or((8, 16));
  usize::from(width_cells.max(1))
    .saturating_mul(usize::from(cell_width.max(1)))
    .saturating_mul(usize::from(height_cells.max(1)))
    .saturating_mul(usize::from(cell_height.max(1)))
    .saturating_mul(4)
}
