//! Render jobs and cached protocol/text output; arguments are named at scheduling time.
use super::{
  PreparedImageCache, RenderKind, RenderedBytes,
  chafa::run_chafa,
  key::{kitty_image_id, kitty_placement_id, render_cache_key},
  memory::prepared_image_estimated_bytes,
};
use crate::{cache, config::RenderConfig, pdf::PageImage, render::RenderedImage};
use img_tui::cache::payload::{CacheHeader, decode_cache_file, encode_cache_file};
use img_tui::{
  EncodedProtocolImage, NativeImageConfig, ProtocolImage, ProtocolImageSpec, RenderMode,
  native_image,
};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub(super) struct RenderJob {
  pub page: PageImage,
  pub width: u16,
  pub height: u16,
  pub kind: RenderKind,
  pub cache_dir: PathBuf,
  pub config: RenderConfig,
  pub native_config: NativeImageConfig,
  pub modes: Vec<RenderMode>,
  pub prepared_images: PreparedImageCache,
}

pub(super) async fn render_with_fallbacks(job: RenderJob) -> Result<RenderedImage, String> {
  let mut errors = Vec::new();
  for &mode in &job.modes {
    let image_id = kitty_image_id(&job.page, job.width, job.height, mode);
    let placement_id = if job.native_config.kitty_unicode_placeholders {
      None
    } else {
      kitty_placement_id(&job.page, mode, image_id)
    };
    let header = CacheHeader {
      width: job.width,
      height: job.height,
      cell_pixels: job.native_config.cell_pixels,
      mode,
      image_id,
      placement_id,
    };
    match render_or_read_cache(&job, &header).await {
      Ok(image) => return Ok(image),
      Err(error) => errors.push(format!("{}: {error}", mode.label())),
    }
  }
  Err(errors.join("; "))
}

async fn read_render_cache(
  path: &std::path::Path,
  header: &CacheHeader,
  native: &NativeImageConfig,
) -> Option<RenderedImage> {
  let bytes = img_tui::cache::paths::read(path).await.ok()?;
  let decoded = decode_cache_file(&bytes, header).await.ok()?;
  let spec = protocol_spec(
    header.mode,
    header.width,
    header.height,
    header.image_id,
    header.placement_id,
  );
  decode_rendered(decoded.payload, &spec, native).ok()
}

async fn render_or_read_cache(
  job: &RenderJob,
  header: &CacheHeader,
) -> Result<RenderedImage, String> {
  let cache_path = job.cache_dir.join(format!(
    "{}.ansi",
    render_cache_key(
      &job.page,
      job.width,
      job.height,
      job.kind,
      &job.config,
      &job.native_config,
      header.mode
    )
  ));
  if let Some(image) = read_render_cache(&cache_path, header, &job.native_config).await {
    return Ok(image);
  }
  let _lock = cache::acquire_cache_file_lock(&cache_path)
    .await
    .map_err(|e| e.to_string())?;
  if let Some(image) = read_render_cache(&cache_path, header, &job.native_config).await {
    return Ok(image);
  }
  let bytes = render_bytes(job, header).await?;
  if let Ok(encoded) = encode_cache_file(
    &bytes,
    header,
    job.config.cache_compression_level,
    job.config.cache_compression_threads,
  )
  .await
    && let Err(error) = cache::write_bytes_atomic(&cache_path, &encoded).await
  {
    tracing::warn!(%error, "failed to store terminal render cache");
  }
  let spec = protocol_spec(
    header.mode,
    header.width,
    header.height,
    header.image_id,
    header.placement_id,
  );
  decode_rendered(bytes, &spec, &job.native_config)
}

async fn render_bytes(job: &RenderJob, header: &CacheHeader) -> Result<RenderedBytes, String> {
  if header.mode.is_protocol() {
    let prepared = prepared_native_image(
      &job.page,
      job.width,
      job.height,
      job.kind,
      &job.native_config,
      &job.prepared_images,
    )
    .await?;
    let spec = protocol_spec(
      header.mode,
      header.width,
      header.height,
      header.image_id,
      header.placement_id,
    );
    native_image::encode_protocol(&prepared, &spec, &job.native_config)
      .await
      .map(|encoded| RenderedBytes {
        data: encoded.data,
        refresh: encoded.refresh,
      })
      .map_err(|e| e.to_string())
  } else {
    run_chafa(
      &job.page.path,
      job.width,
      job.height,
      &job.config,
      header.mode,
    )
    .await
    .map(|data| RenderedBytes {
      data,
      refresh: None,
    })
  }
}

fn prepared_dimensions(_kind: RenderKind, width: u16, height: u16) -> (u16, u16) {
  (width, height)
}

fn prepared_cache_key(
  page: &PageImage,
  width: u16,
  height: u16,
  kind: RenderKind,
  native_config: &NativeImageConfig,
) -> String {
  let (prepared_width, prepared_height) = prepared_dimensions(kind, width, height);
  let mut hasher = Sha256::new();
  hasher.update(b"pdf-tui-prepared-image-v2");
  hasher.update(page.path.to_string_lossy().as_bytes());
  hasher.update(page.page_index.to_le_bytes());
  hasher.update(page.size_bytes.to_le_bytes());
  hasher.update(page.modified_nanos.to_le_bytes());
  hasher.update(prepared_width.to_le_bytes());
  hasher.update(prepared_height.to_le_bytes());
  hasher.update(native_config.cell_pixels.unwrap_or((0, 0)).0.to_le_bytes());
  hasher.update(native_config.cell_pixels.unwrap_or((0, 0)).1.to_le_bytes());
  hex::encode(hasher.finalize())
}

async fn prepared_native_image(
  page: &PageImage,
  width: u16,
  height: u16,
  kind: RenderKind,
  native_config: &NativeImageConfig,
  cache: &PreparedImageCache,
) -> Result<native_image::PreparedNativeImage, String> {
  let key = prepared_cache_key(page, width, height, kind, native_config);
  {
    let mut cache = cache.lock().await;
    if let Some(prepared) = cache.get(&key) {
      return Ok(prepared);
    }
  }

  let (prepared_width, prepared_height) = prepared_dimensions(kind, width, height);
  let prepared = native_image::prepare(
    &page.path,
    prepared_width,
    prepared_height,
    native_config.cell_pixels,
  )
  .await
  .map_err(|error| error.to_string())?;
  let bytes =
    prepared_image_estimated_bytes(prepared_width, prepared_height, native_config.cell_pixels);
  let mut cache = cache.lock().await;
  if let Some(existing) = cache.get(&key) {
    return Ok(existing);
  }
  cache.insert(key, prepared.clone(), bytes);
  Ok(prepared)
}

/// How a render is encoded (protocol modes) and read back.
fn protocol_spec(
  mode: RenderMode,
  width: u16,
  height: u16,
  image_id: Option<u32>,
  placement_id: Option<u32>,
) -> ProtocolImageSpec {
  ProtocolImageSpec {
    image_id,
    placement_id,
    ..ProtocolImageSpec::new(mode, width, height)
  }
}

fn decode_rendered(
  bytes: RenderedBytes,
  spec: &ProtocolImageSpec,
  native_config: &NativeImageConfig,
) -> Result<RenderedImage, String> {
  if spec.mode.is_protocol() {
    let encoded = EncodedProtocolImage {
      data: bytes.data,
      refresh: bytes.refresh,
    };
    ProtocolImage::from_encoded(encoded, spec, native_config)
      .map(RenderedImage::Protocol)
      .map_err(|error| error.to_string())
  } else {
    let text = img_tui::chafa::decode(&bytes.data)?;
    Ok(RenderedImage::Symbols { text })
  }
}
