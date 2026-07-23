use std::{
  io::{Cursor, Write},
  path::Path,
};

use img_tui::RenderMode;
use tokio::fs;

use crate::{cache, config::RenderConfig};

use super::RenderedBytes;

const CACHE_MAGIC: &str = "pdf-tui-render-cache-v4";
const LEGACY_ZSTD_CACHE_MAGIC: &str = "pdf-tui-render-cache-v2";
const LEGACY_RAW_CACHE_MAGIC: &str = "pdf-tui-render-cache-v1";
const FRAMED_PAYLOAD_MAGIC: &[u8] = b"pdf-tui-rendered-bytes-v1\0";

pub(super) struct DecodedCacheFile {
  pub(super) payload: RenderedBytes,
  pub(super) image_id: Option<u32>,
  pub(super) placement_id: Option<u32>,
  pub(super) should_rewrite: bool,
}

struct CacheFileMetadata {
  width: u16,
  height: u16,
  cell_pixels: Option<(u16, u16)>,
  mode: RenderMode,
  image_id: Option<u32>,
  placement_id: Option<u32>,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn write_cache_file(
  cache_path: &Path,
  payload: &RenderedBytes,
  width: u16,
  height: u16,
  cell_pixels: Option<(u16, u16)>,
  mode: RenderMode,
  image_id: Option<u32>,
  placement_id: Option<u32>,
  config: &RenderConfig,
) -> Result<(), String> {
  if let Some(parent) = cache_path.parent() {
    fs::create_dir_all(parent)
      .await
      .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
  }
  let cached = encode_cache_file(
    payload,
    CacheFileMetadata {
      width,
      height,
      cell_pixels,
      mode,
      image_id,
      placement_id,
    },
    config,
  )
  .await
  .map_err(|error| format!("failed to encode cache {}: {error}", cache_path.display()))?;
  cache::write_bytes_atomic(cache_path, &cached)
    .await
    .map_err(|error| format!("failed to write cache {}: {error}", cache_path.display()))?;
  cache::touch_cache_entry(cache_path).await;
  Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn rewrite_cache_file(
  cache_path: &Path,
  payload: &RenderedBytes,
  width: u16,
  height: u16,
  cell_pixels: Option<(u16, u16)>,
  mode: RenderMode,
  image_id: Option<u32>,
  placement_id: Option<u32>,
  config: &RenderConfig,
) {
  let _ = write_cache_file(
    cache_path,
    payload,
    width,
    height,
    cell_pixels,
    mode,
    image_id,
    placement_id,
    config,
  )
  .await;
}

async fn encode_cache_file(
  payload: &RenderedBytes,
  metadata: CacheFileMetadata,
  config: &RenderConfig,
) -> Result<Vec<u8>, String> {
  let compression_level = config.cache_compression_level;
  let compression_threads = config.cache_compression_threads;
  let payload_format = if payload.refresh.is_some() {
    "framed"
  } else {
    "raw"
  };
  let payload = encode_rendered_bytes(payload)?;
  let plain_len = payload.len();
  let compressed = tokio::task::spawn_blocking(move || {
    compress_zstd(payload, compression_level, compression_threads)
  })
  .await
  .map_err(|error| format!("zstd compression worker failed: {error}"))?
  .map_err(|error| format!("zstd compression failed: {error}"))?;

  let (cell_width, cell_height) = metadata.cell_pixels.unwrap_or((0, 0));
  let width = metadata.width;
  let height = metadata.height;
  let mut header = format!(
    "{CACHE_MAGIC}\nwidth={width}\nheight={height}\ncell_width={cell_width}\ncell_height={cell_height}\nmode={}\ncompression=zstd\npayload_format={payload_format}\nuncompressed_bytes={plain_len}\n",
    metadata.mode.label()
  );
  if let Some(image_id) = metadata.image_id {
    header.push_str(&format!("image_id={image_id}\n"));
  }
  if let Some(placement_id) = metadata.placement_id {
    header.push_str(&format!("placement_id={placement_id}\n"));
  }
  header.push('\n');
  let mut out = Vec::with_capacity(header.len() + compressed.len());
  out.extend_from_slice(header.as_bytes());
  out.extend_from_slice(&compressed);
  Ok(out)
}

pub(super) async fn decode_cache_file(
  bytes: &[u8],
  expected_width: u16,
  expected_height: u16,
  expected_cell_pixels: Option<(u16, u16)>,
  expected_mode: RenderMode,
  expected_image_id: Option<u32>,
  expected_placement_id: Option<u32>,
) -> Result<DecodedCacheFile, String> {
  let header_end = bytes
    .windows(2)
    .position(|window| window == b"\n\n")
    .ok_or_else(|| "cache metadata header missing".to_string())?;
  let header = std::str::from_utf8(&bytes[..header_end])
    .map_err(|error| format!("cache metadata is not utf-8: {error}"))?;
  let mut lines = header.lines();
  let magic = lines
    .next()
    .ok_or_else(|| "cache metadata magic missing".to_string())?;
  if magic != CACHE_MAGIC && magic != LEGACY_ZSTD_CACHE_MAGIC && magic != LEGACY_RAW_CACHE_MAGIC {
    return Err("cache metadata magic mismatch".to_string());
  }

  let mut width = None;
  let mut height = None;
  let mut cell_width = None;
  let mut cell_height = None;
  let mut mode = None;
  let mut compression = None;
  let mut payload_format = None;
  let mut uncompressed_bytes = None;
  let mut image_id = None;
  let mut placement_id = None;
  for line in lines {
    if let Some(value) = line.strip_prefix("width=") {
      width = value.parse::<u16>().ok();
    } else if let Some(value) = line.strip_prefix("height=") {
      height = value.parse::<u16>().ok();
    } else if let Some(value) = line.strip_prefix("cell_width=") {
      cell_width = value.parse::<u16>().ok();
    } else if let Some(value) = line.strip_prefix("cell_height=") {
      cell_height = value.parse::<u16>().ok();
    } else if let Some(value) = line.strip_prefix("mode=") {
      mode = Some(value);
    } else if let Some(value) = line.strip_prefix("compression=") {
      compression = Some(value);
    } else if let Some(value) = line.strip_prefix("payload_format=") {
      payload_format = Some(value);
    } else if let Some(value) = line.strip_prefix("uncompressed_bytes=") {
      uncompressed_bytes = value.parse::<usize>().ok();
    } else if let Some(value) = line.strip_prefix("image_id=") {
      image_id = value.parse::<u32>().ok();
    } else if let Some(value) = line.strip_prefix("placement_id=") {
      placement_id = value.parse::<u32>().ok();
    }
  }

  if width != Some(expected_width) || height != Some(expected_height) {
    return Err(format!(
      "cache size mismatch: got {:?}x{:?}, expected {}x{}",
      width, height, expected_width, expected_height
    ));
  }
  if mode != Some(expected_mode.label()) {
    return Err(format!(
      "cache mode mismatch: got {:?}, expected {}",
      mode,
      expected_mode.label()
    ));
  }
  let (expected_cell_width, expected_cell_height) = expected_cell_pixels.unwrap_or((0, 0));
  if cell_width != Some(expected_cell_width) || cell_height != Some(expected_cell_height) {
    return Err(format!(
      "cache cell size mismatch: got {:?}x{:?}, expected {}x{}",
      cell_width, cell_height, expected_cell_width, expected_cell_height
    ));
  }
  if image_id != expected_image_id {
    return Err(format!(
      "cache image id mismatch: got {:?}, expected {:?}",
      image_id, expected_image_id
    ));
  }
  if placement_id != expected_placement_id {
    return Err(format!(
      "cache placement id mismatch: got {:?}, expected {:?}",
      placement_id, expected_placement_id
    ));
  }

  let payload = &bytes[header_end + 2..];
  let payload_format = payload_format.unwrap_or("raw");
  match compression.unwrap_or("none") {
    "none" => Ok(DecodedCacheFile {
      payload: decode_rendered_bytes(payload.to_vec(), payload_format)?,
      image_id,
      placement_id,
      should_rewrite: magic != CACHE_MAGIC || payload_format != "raw",
    }),
    "zstd" => {
      let expected_len = uncompressed_bytes;
      let payload = payload.to_vec();
      let decoded = tokio::task::spawn_blocking(move || decompress_zstd(payload))
        .await
        .map_err(|error| format!("zstd decompression worker failed: {error}"))?
        .map_err(|error| format!("zstd decompression failed: {error}"))?;
      if let Some(expected_len) = expected_len
        && decoded.len() != expected_len
      {
        return Err(format!(
          "cache decompressed size mismatch: got {}, expected {}",
          decoded.len(),
          expected_len
        ));
      }
      Ok(DecodedCacheFile {
        payload: decode_rendered_bytes(decoded, payload_format)?,
        image_id,
        placement_id,
        should_rewrite: magic != CACHE_MAGIC,
      })
    }
    value => Err(format!("unsupported cache compression: {value}")),
  }
}

fn compress_zstd(payload: Vec<u8>, level: i32, threads: u32) -> std::io::Result<Vec<u8>> {
  let mut encoder = zstd::stream::Encoder::new(Vec::new(), level)?;
  if threads > 0 {
    encoder.multithread(threads)?;
  }
  encoder.write_all(&payload)?;
  encoder.finish()
}

fn decompress_zstd(payload: Vec<u8>) -> std::io::Result<Vec<u8>> {
  zstd::stream::decode_all(Cursor::new(payload))
}

fn encode_rendered_bytes(payload: &RenderedBytes) -> Result<Vec<u8>, String> {
  let Some(refresh) = &payload.refresh else {
    return Ok(payload.data.clone());
  };
  let data_len = u64::try_from(payload.data.len())
    .map_err(|_| "render payload is too large to cache".to_string())?;
  let refresh_len = u64::try_from(refresh.len())
    .map_err(|_| "refresh payload is too large to cache".to_string())?;
  let mut out = Vec::with_capacity(
    FRAMED_PAYLOAD_MAGIC.len() + 16 + payload.data.len().saturating_add(refresh.len()),
  );
  out.extend_from_slice(FRAMED_PAYLOAD_MAGIC);
  out.extend_from_slice(&data_len.to_le_bytes());
  out.extend_from_slice(&refresh_len.to_le_bytes());
  out.extend_from_slice(&payload.data);
  out.extend_from_slice(refresh);
  Ok(out)
}

fn decode_rendered_bytes(bytes: Vec<u8>, payload_format: &str) -> Result<RenderedBytes, String> {
  if payload_format != "framed" {
    return Ok(RenderedBytes {
      data: bytes,
      refresh: None,
    });
  }
  let header_len = FRAMED_PAYLOAD_MAGIC.len() + 16;
  if bytes.len() < header_len || !bytes.starts_with(FRAMED_PAYLOAD_MAGIC) {
    return Err("framed render payload magic mismatch".to_string());
  }
  let lengths = &bytes[FRAMED_PAYLOAD_MAGIC.len()..header_len];
  let data_len = u64::from_le_bytes(
    lengths[0..8]
      .try_into()
      .map_err(|_| "render payload data length missing".to_string())?,
  );
  let refresh_len = u64::from_le_bytes(
    lengths[8..16]
      .try_into()
      .map_err(|_| "render payload refresh length missing".to_string())?,
  );
  let data_len =
    usize::try_from(data_len).map_err(|_| "render payload data length is too large".to_string())?;
  let refresh_len = usize::try_from(refresh_len)
    .map_err(|_| "render payload refresh length is too large".to_string())?;
  let data_start = header_len;
  let refresh_start = data_start.saturating_add(data_len);
  let end = refresh_start.saturating_add(refresh_len);
  if end != bytes.len() {
    return Err(format!(
      "framed render payload size mismatch: got {}, expected {}",
      bytes.len(),
      end
    ));
  }
  Ok(RenderedBytes {
    data: bytes[data_start..refresh_start].to_vec(),
    refresh: Some(bytes[refresh_start..end].to_vec()),
  })
}
