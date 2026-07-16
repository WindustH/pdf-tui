use std::{
  collections::{HashMap, VecDeque},
  io::Cursor,
};

use img_tui::{ProtocolPlacement, RenderMode, native_image};

use crate::event::RenderedImage;

#[derive(Debug)]
struct RenderedImageEntry {
  storage: RenderedImageStorage,
  stored_bytes: usize,
  raw_bytes: usize,
}

#[derive(Debug)]
enum RenderedImageStorage {
  Raw(RenderedImage),
  Compressed(CompressedRenderedImage),
}

#[derive(Debug)]
struct CompressedRenderedImage {
  mode: RenderMode,
  data: Vec<u8>,
  refresh: Option<Vec<u8>>,
  placement: Option<ProtocolPlacement>,
  fingerprint: u64,
  erase: Option<Vec<u8>>,
}

#[derive(Debug)]
pub(super) struct RenderedImageMemoryCache {
  entries: HashMap<String, RenderedImageEntry>,
  order: VecDeque<String>,
  raw_bytes: usize,
  compressed_bytes: usize,
  raw_max_bytes: usize,
  compressed_max_bytes: usize,
  compression: bool,
}

impl RenderedImageMemoryCache {
  pub(super) fn new(raw_max_bytes: usize, compressed_max_bytes: usize, compression: bool) -> Self {
    Self {
      entries: HashMap::new(),
      order: VecDeque::new(),
      raw_bytes: 0,
      compressed_bytes: 0,
      raw_max_bytes: raw_max_bytes.max(1),
      compressed_max_bytes: compressed_max_bytes.max(1),
      compression,
    }
  }

  pub(super) fn clear(&mut self) {
    self.entries.clear();
    self.order.clear();
    self.raw_bytes = 0;
    self.compressed_bytes = 0;
  }

  pub(super) fn contains_key(&self, key: &str) -> bool {
    self.entries.contains_key(key)
  }

  pub(super) fn get(&mut self, key: &str) -> Option<&RenderedImage> {
    if !self.entries.contains_key(key) {
      return None;
    }
    if self
      .entries
      .get(key)
      .is_some_and(|entry| matches!(entry.storage, RenderedImageStorage::Compressed(_)))
    {
      let image = self
        .entries
        .get(key)
        .and_then(|entry| match &entry.storage {
          RenderedImageStorage::Compressed(compressed) => compressed.decompress().ok(),
          RenderedImageStorage::Raw(_) => None,
        })?;
      self.promote_to_raw(key, image);
    }
    self.touch(key);
    self
      .entries
      .get(key)
      .and_then(|entry| match &entry.storage {
        RenderedImageStorage::Raw(image) => Some(image),
        RenderedImageStorage::Compressed(_) => None,
      })
  }

  pub(super) fn touch(&mut self, key: &str) {
    if !self.entries.contains_key(key) {
      return;
    }
    self.order.retain(|candidate| candidate != key);
    self.order.push_back(key.to_string());
  }

  pub(super) fn insert(&mut self, key: String, image: RenderedImage) -> Vec<String> {
    if let Some(old) = self.entries.remove(&key) {
      self.subtract_entry_bytes(&old);
      self.order.retain(|candidate| candidate != &key);
    }
    let bytes = rendered_image_bytes(&image);
    self.raw_bytes = self.raw_bytes.saturating_add(bytes);
    self.order.push_back(key.clone());
    self.entries.insert(
      key.clone(),
      RenderedImageEntry {
        storage: RenderedImageStorage::Raw(image),
        stored_bytes: bytes,
        raw_bytes: bytes,
      },
    );
    self.evict_over_budget(&key)
  }

  fn evict_over_budget(&mut self, protected_key: &str) -> Vec<String> {
    if self.compression {
      self.compress_cold_raw_entries(protected_key);
    }
    let mut evicted = Vec::new();
    self.evict_compressed_over_budget(protected_key, &mut evicted);
    self.evict_raw_over_budget(protected_key, &mut evicted);
    evicted
  }

  fn compress_cold_raw_entries(&mut self, protected_key: &str) {
    if self.raw_bytes <= self.raw_max_bytes {
      return;
    }
    let keys = self.order.iter().cloned().collect::<Vec<_>>();
    for key in keys {
      if self.raw_bytes <= self.raw_max_bytes {
        break;
      }
      if key == protected_key {
        continue;
      }
      let Some(entry) = self.entries.get_mut(&key) else {
        continue;
      };
      let RenderedImageStorage::Raw(image) = &entry.storage else {
        continue;
      };
      let Some(compressed) = CompressedRenderedImage::compress(image) else {
        continue;
      };
      let stored_bytes = compressed.stored_bytes();
      self.raw_bytes = self.raw_bytes.saturating_sub(entry.raw_bytes);
      self.compressed_bytes = self.compressed_bytes.saturating_add(stored_bytes);
      entry.storage = RenderedImageStorage::Compressed(compressed);
      entry.stored_bytes = stored_bytes;
    }
  }

  fn promote_to_raw(&mut self, key: &str, image: RenderedImage) {
    let Some(entry) = self.entries.get_mut(key) else {
      return;
    };
    let old_stored_bytes = entry.stored_bytes;
    let old_was_raw = matches!(entry.storage, RenderedImageStorage::Raw(_));
    let raw_bytes = rendered_image_bytes(&image);
    entry.storage = RenderedImageStorage::Raw(image);
    entry.stored_bytes = raw_bytes;
    entry.raw_bytes = raw_bytes;
    if old_was_raw {
      self.raw_bytes = self
        .raw_bytes
        .saturating_sub(old_stored_bytes)
        .saturating_add(raw_bytes);
    } else {
      self.compressed_bytes = self.compressed_bytes.saturating_sub(old_stored_bytes);
      self.raw_bytes = self.raw_bytes.saturating_add(raw_bytes);
    }
    self.evict_over_budget(key);
  }

  fn evict_compressed_over_budget(&mut self, protected_key: &str, evicted: &mut Vec<String>) {
    while self.compressed_bytes > self.compressed_max_bytes && self.entries.len() > 1 {
      let Some(candidate) = self.pop_oldest_evictable_matching(protected_key, |entry| {
        matches!(entry.storage, RenderedImageStorage::Compressed(_))
      }) else {
        break;
      };
      let Some(entry) = self.entries.remove(&candidate) else {
        continue;
      };
      self.subtract_entry_bytes(&entry);
      evicted.push(candidate);
    }
  }

  fn evict_raw_over_budget(&mut self, protected_key: &str, evicted: &mut Vec<String>) {
    while self.raw_bytes > self.raw_max_bytes && self.entries.len() > 1 {
      let Some(candidate) = self.pop_oldest_evictable_matching(protected_key, |entry| {
        matches!(entry.storage, RenderedImageStorage::Raw(_))
      }) else {
        break;
      };
      let Some(entry) = self.entries.remove(&candidate) else {
        continue;
      };
      self.subtract_entry_bytes(&entry);
      evicted.push(candidate);
    }
  }

  fn pop_oldest_evictable_matching(
    &mut self,
    protected_key: &str,
    matches_entry: impl Fn(&RenderedImageEntry) -> bool,
  ) -> Option<String> {
    let initial_len = self.order.len();
    for _ in 0..initial_len {
      let candidate = self.order.pop_front()?;
      if candidate == protected_key {
        self.order.push_back(candidate);
        continue;
      }
      if !self.entries.get(&candidate).is_some_and(&matches_entry) {
        self.order.push_back(candidate);
        continue;
      }
      return Some(candidate);
    }
    None
  }

  fn subtract_entry_bytes(&mut self, entry: &RenderedImageEntry) {
    match &entry.storage {
      RenderedImageStorage::Raw(_) => {
        self.raw_bytes = self.raw_bytes.saturating_sub(entry.raw_bytes);
      }
      RenderedImageStorage::Compressed(_) => {
        self.compressed_bytes = self.compressed_bytes.saturating_sub(entry.stored_bytes);
      }
    }
  }
}

impl CompressedRenderedImage {
  fn compress(image: &RenderedImage) -> Option<Self> {
    let RenderedImage::Protocol {
      mode,
      data,
      refresh,
      placement,
      fingerprint,
      erase,
    } = image
    else {
      return None;
    };
    Some(Self {
      mode: *mode,
      data: compress_memory_bytes(data.as_bytes()).ok()?,
      refresh: refresh
        .as_ref()
        .map(|refresh| compress_memory_bytes(refresh.as_bytes()))
        .transpose()
        .ok()?,
      placement: placement.clone(),
      fingerprint: *fingerprint,
      erase: erase
        .as_ref()
        .map(|erase| compress_memory_bytes(erase.as_bytes()))
        .transpose()
        .ok()?,
    })
  }

  fn decompress(&self) -> Result<RenderedImage, String> {
    Ok(RenderedImage::Protocol {
      mode: self.mode,
      data: String::from_utf8(decompress_memory_bytes(&self.data)?)
        .map_err(|error| error.to_string())?,
      refresh: self
        .refresh
        .as_ref()
        .map(|refresh| {
          String::from_utf8(decompress_memory_bytes(refresh)?).map_err(|error| error.to_string())
        })
        .transpose()?,
      placement: self.placement.clone(),
      fingerprint: self.fingerprint,
      erase: self
        .erase
        .as_ref()
        .map(|erase| {
          String::from_utf8(decompress_memory_bytes(erase)?).map_err(|error| error.to_string())
        })
        .transpose()?,
    })
  }

  fn stored_bytes(&self) -> usize {
    self
      .data
      .len()
      .saturating_add(self.refresh.as_ref().map_or(0, Vec::len))
      .saturating_add(self.erase.as_ref().map_or(0, Vec::len))
      .saturating_add(256)
  }
}

#[derive(Debug, Clone)]
pub(super) struct PreparedImageCacheEntry {
  image: native_image::PreparedNativeImage,
  bytes: usize,
}

#[derive(Debug)]
pub(super) struct PreparedImageMemoryCache {
  entries: HashMap<String, PreparedImageCacheEntry>,
  order: VecDeque<String>,
  bytes: usize,
  max_bytes: usize,
}

impl PreparedImageMemoryCache {
  pub(super) fn new(max_bytes: usize) -> Self {
    Self {
      entries: HashMap::new(),
      order: VecDeque::new(),
      bytes: 0,
      max_bytes: max_bytes.max(1),
    }
  }

  pub(super) fn get(&mut self, key: &str) -> Option<native_image::PreparedNativeImage> {
    if !self.entries.contains_key(key) {
      return None;
    }
    self.touch(key);
    self.entries.get(key).map(|entry| entry.image.clone())
  }

  pub(super) fn insert(
    &mut self,
    key: String,
    image: native_image::PreparedNativeImage,
    bytes: usize,
  ) {
    if let Some(old) = self.entries.remove(&key) {
      self.bytes = self.bytes.saturating_sub(old.bytes);
      self.order.retain(|candidate| candidate != &key);
    }
    let bytes = bytes.max(1);
    self.bytes = self.bytes.saturating_add(bytes);
    self.order.push_back(key.clone());
    self
      .entries
      .insert(key.clone(), PreparedImageCacheEntry { image, bytes });
    self.evict_over_budget(&key);
  }

  fn touch(&mut self, key: &str) {
    if !self.entries.contains_key(key) {
      return;
    }
    self.order.retain(|candidate| candidate != key);
    self.order.push_back(key.to_string());
  }

  fn evict_over_budget(&mut self, protected_key: &str) {
    while self.bytes > self.max_bytes && self.entries.len() > 1 {
      let Some(candidate) = self.order.pop_front() else {
        break;
      };
      if candidate == protected_key {
        self.order.push_back(candidate);
        if self.order.len() <= 1 {
          break;
        }
        continue;
      }
      if let Some(entry) = self.entries.remove(&candidate) {
        self.bytes = self.bytes.saturating_sub(entry.bytes);
      }
    }
  }
}

pub(super) fn memory_cache_bytes(configured: u64) -> usize {
  usize::try_from(configured).unwrap_or(usize::MAX).max(1)
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
    RenderedImage::Protocol {
      data,
      refresh,
      erase,
      ..
    } => data
      .len()
      .saturating_add(refresh.as_ref().map_or(0, String::len))
      .saturating_add(erase.as_ref().map_or(0, String::len))
      .saturating_add(256),
  }
}

fn compress_memory_bytes(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
  zstd::stream::encode_all(Cursor::new(bytes), 1)
}

fn decompress_memory_bytes(bytes: &[u8]) -> Result<Vec<u8>, String> {
  zstd::stream::decode_all(Cursor::new(bytes))
    .map_err(|error| format!("memory cache decompression failed: {error}"))
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
