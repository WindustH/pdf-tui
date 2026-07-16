use std::{
  path::{Path, PathBuf},
  sync::Arc,
};

use ansi_to_tui::IntoText;
use img_tui::{NativeImageConfig, ProtocolPlacement, RenderMode, native_image};
use ratatui::text::Text;
use sha2::{Digest, Sha256};
use tokio::{fs, sync::Semaphore};

use crate::{cache, config::RenderConfig, event::RenderedImage, pdf::PageImage};

use super::{
  PreparedImageCache, RenderKind, RenderPermits, RenderedBytes,
  cache_file::{decode_cache_file, rewrite_cache_file, write_cache_file},
  chafa::run_chafa,
  key::{kitty_image_id, kitty_placement_id, render_cache_key, render_fingerprint},
  memory::prepared_image_estimated_bytes,
};

#[allow(clippy::too_many_arguments)]
pub(super) async fn render_with_fallbacks(
  page: PageImage,
  width: u16,
  height: u16,
  kind: RenderKind,
  cache_dir: PathBuf,
  config: RenderConfig,
  native_config: NativeImageConfig,
  modes: Vec<RenderMode>,
  prepared_images: PreparedImageCache,
  semaphore: Arc<Semaphore>,
  permits: Option<RenderPermits>,
) -> Result<RenderedImage, String> {
  let _permits = acquire_render_permits(semaphore, permits).await?;

  let mut errors = Vec::new();
  for mode in modes {
    let image_id = kitty_image_id(&page, width, height, mode);
    let placement_id = kitty_placement_id(&page, mode, image_id);
    let rendered = render_or_read_cache(
      &page,
      &cache_dir,
      width,
      height,
      kind,
      &config,
      &native_config,
      mode,
      image_id,
      placement_id,
      &prepared_images,
    )
    .await;
    match rendered {
      Ok(rendered) => return Ok(rendered),
      Err(error) => errors.push(format!("{}: {error}", mode.label())),
    }
  }
  Err(errors.join("; "))
}

async fn acquire_render_permits(
  semaphore: Arc<Semaphore>,
  permits: Option<RenderPermits>,
) -> Result<RenderPermits, String> {
  match permits {
    Some(permits) => Ok(permits),
    None => Ok(RenderPermits {
      _global: semaphore
        .acquire_owned()
        .await
        .map_err(|error| error.to_string())?,
      _preload: None,
    }),
  }
}

#[allow(clippy::too_many_arguments)]
async fn render_or_read_cache(
  page: &PageImage,
  cache_dir: &Path,
  width: u16,
  height: u16,
  kind: RenderKind,
  config: &RenderConfig,
  native_config: &NativeImageConfig,
  mode: RenderMode,
  image_id: Option<u32>,
  placement_id: Option<u32>,
  prepared_images: &PreparedImageCache,
) -> Result<RenderedImage, String> {
  let cache_path = cache_dir.join(format!(
    "{}.ansi",
    render_cache_key(page, width, height, kind, config, native_config, mode)
  ));

  if let Ok(bytes) = fs::read(&cache_path).await {
    if let Ok(decoded) = decode_cache_file(
      &bytes,
      width,
      height,
      native_config.cell_pixels,
      mode,
      image_id,
      placement_id,
    )
    .await
    {
      if decoded.should_rewrite {
        rewrite_cache_file(
          &cache_path,
          &decoded.payload,
          width,
          height,
          native_config.cell_pixels,
          mode,
          image_id,
          placement_id,
          config,
        )
        .await;
      }
      cache::touch_cache_entry(&cache_path).await;
      return decode_rendered(
        decoded.payload,
        mode,
        native_config,
        decoded.image_id,
        decoded.placement_id,
      );
    }
  }

  let bytes = render_bytes(
    page,
    width,
    height,
    kind,
    config,
    native_config,
    mode,
    image_id,
    placement_id,
    prepared_images,
  )
  .await?;

  let _ = write_cache_file(
    &cache_path,
    &bytes,
    width,
    height,
    native_config.cell_pixels,
    mode,
    image_id,
    placement_id,
    config,
  )
  .await;

  decode_rendered(bytes, mode, native_config, image_id, placement_id)
}

#[allow(clippy::too_many_arguments)]
async fn render_bytes(
  page: &PageImage,
  width: u16,
  height: u16,
  kind: RenderKind,
  config: &RenderConfig,
  native_config: &NativeImageConfig,
  mode: RenderMode,
  image_id: Option<u32>,
  placement_id: Option<u32>,
  prepared_images: &PreparedImageCache,
) -> Result<RenderedBytes, String> {
  let source_path = page.path.clone();
  if mode.is_protocol() {
    let prepared =
      prepared_native_image(page, width, height, kind, native_config, prepared_images).await?;
    if mode == RenderMode::Kitty
      && let Some(placement_id) = placement_id
    {
      let viewport = native_image::NativeImageViewport {
        full_width_cells: width,
        full_height_cells: height,
        visible_width_cells: width,
        visible_height_cells: height,
        left_cells: 0,
        top_cells: 0,
      };
      let image_id = image_id.unwrap_or(1);
      let upload = native_image::render_prepared_kitty_upload(&prepared, native_config, image_id)
        .await
        .map_err(|error| error.to_string())?;
      let refresh = native_image::render_kitty_viewport_from_upload(
        &upload,
        viewport,
        native_config,
        placement_id,
      )
      .map_err(|error| error.to_string())?;
      Ok(RenderedBytes {
        data: upload.data,
        refresh: Some(refresh),
      })
    } else {
      native_image::render_prepared(&prepared, mode, native_config, image_id)
        .await
        .map(|data| RenderedBytes {
          data,
          refresh: None,
        })
        .map_err(|error| error.to_string())
    }
  } else {
    run_chafa(&source_path, width, height, config, mode)
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

fn decode_rendered(
  bytes: RenderedBytes,
  mode: RenderMode,
  native_config: &NativeImageConfig,
  image_id: Option<u32>,
  placement_id: Option<u32>,
) -> Result<RenderedImage, String> {
  decode_rendered_with_refresh(bytes, mode, native_config, image_id, placement_id)
}

fn decode_rendered_with_refresh(
  bytes: RenderedBytes,
  mode: RenderMode,
  native_config: &NativeImageConfig,
  image_id: Option<u32>,
  placement_id: Option<u32>,
) -> Result<RenderedImage, String> {
  if mode.is_protocol() {
    let fingerprint = render_fingerprint(&bytes.data);
    let data = String::from_utf8(bytes.data).map_err(|error| error.to_string())?;
    let refresh = bytes
      .refresh
      .map(String::from_utf8)
      .transpose()
      .map_err(|error| error.to_string())?;
    let placement = match (
      mode,
      native_config.kitty_unicode_placeholders,
      image_id,
      placement_id,
    ) {
      (RenderMode::Kitty, _, Some(image_id), Some(placement_id)) => {
        Some(ProtocolPlacement::KittyPlacement {
          image_id,
          placement_id,
        })
      }
      (RenderMode::Kitty, true, Some(image_id), None) => {
        Some(ProtocolPlacement::KittyUnicode { image_id })
      }
      _ => None,
    };
    let erase = if mode == RenderMode::Kitty
      && let (Some(image_id), Some(placement_id)) = (image_id, placement_id)
    {
      native_image::erase_kitty_placement_sequence(
        native_config.passthrough.as_deref(),
        image_id,
        placement_id,
      )
    } else {
      native_image::erase_sequence(mode, native_config.passthrough.as_deref(), image_id)
    };
    Ok(RenderedImage::Protocol {
      mode,
      data,
      refresh,
      placement,
      fingerprint,
      erase,
    })
  } else {
    let text: Text<'static> = bytes.data.into_text().map_err(|error| error.to_string())?;
    Ok(RenderedImage::Symbols { mode, text })
  }
}
