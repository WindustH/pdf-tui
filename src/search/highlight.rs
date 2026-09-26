//! Search-match highlighting: a copy of the page (or slice) PNG with the
//! matched rectangle inverted, cached under `<cache>/search-highlight/`.

use std::{fs, path::Path, time::UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::{cache, pdf::PageImage};

use super::PdfSearchMatch;

/// Whether `search_match` covers any pixel of `page` (a whole page or one
/// scroll slice).
pub fn highlight_applies(page: &PageImage, search_match: &PdfSearchMatch) -> bool {
  page.page_index == search_match.page_index && highlight_pixel_rect(page, search_match).is_some()
}

/// Highlighted copy of `page`, or `None` when the match is on another page
/// or outside the rendered region (e.g. a different scroll slice).
pub fn highlighted_viewer_image(
  cache_dir: &Path,
  page: &PageImage,
  search_match: &PdfSearchMatch,
  max_bytes: u64,
) -> Result<Option<PageImage>, String> {
  if !highlight_applies(page, search_match) {
    return Ok(None);
  }
  let dir = cache_dir.join("search-highlight");
  fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
  let cache_key = highlighted_cache_key(page, search_match);
  let path = dir.join(format!("{cache_key}.png"));
  if !path.exists() {
    let _lock = cache::acquire_cache_file_lock_sync(&path).map_err(|error| error.to_string())?;
    if path.exists() {
      cache::touch_cache_entry_sync(&path);
      return page_image_from_highlight_path(page, path).map(Some);
    }
    write_highlighted_page(&path, page, search_match)?;
    let _ = cache::enforce_cache_target_limit_sync(cache_dir, &dir, max_bytes);
  }
  cache::touch_cache_entry_sync(&path);
  page_image_from_highlight_path(page, path).map(Some)
}

fn page_image_from_highlight_path(
  page: &PageImage,
  path: std::path::PathBuf,
) -> Result<PageImage, String> {
  let metadata = fs::metadata(&path).map_err(|err| err.to_string())?;
  Ok(PageImage {
    page_index: page.page_index,
    path,
    width: page.width,
    height: page.height,
    size_bytes: metadata.len(),
    modified_nanos: metadata
      .modified()
      .ok()
      .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
      .map(|duration| duration.as_nanos())
      .unwrap_or_default(),
    slice: page.slice.clone(),
  })
}

fn write_highlighted_page(
  path: &Path,
  page: &PageImage,
  search_match: &PdfSearchMatch,
) -> Result<(), String> {
  let image = image::open(&page.path)
    .map_err(|err| format!("failed to open {}: {err}", page.path.display()))?;
  let mut image = image.to_rgba8();
  let Some((x0, y0, x1, y1)) = highlight_pixel_rect(page, search_match) else {
    return Ok(());
  };
  for y in y0..y1 {
    for x in x0..x1 {
      let pixel = image.get_pixel_mut(x, y);
      pixel.0[0] = 255u8.saturating_sub(pixel.0[0]);
      pixel.0[1] = 255u8.saturating_sub(pixel.0[1]);
      pixel.0[2] = 255u8.saturating_sub(pixel.0[2]);
    }
  }
  cache::write_file_atomic_sync(path, |temp| {
    image.save_with_format(temp, image::ImageFormat::Png)
  })
}

fn highlighted_cache_key(page: &PageImage, search_match: &PdfSearchMatch) -> String {
  let mut hasher = Sha256::new();
  hasher.update(b"pdf-tui-search-highlight-v1");
  hasher.update(page.path.to_string_lossy().as_bytes());
  hasher.update(page.size_bytes.to_le_bytes());
  hasher.update(page.modified_nanos.to_le_bytes());
  hasher.update(page.width.to_le_bytes());
  hasher.update(page.height.to_le_bytes());
  hasher.update(search_match.page_index.to_le_bytes());
  for value in [
    search_match.rect.x_min,
    search_match.rect.y_min,
    search_match.rect.x_max,
    search_match.rect.y_max,
    search_match.page_width,
    search_match.page_height,
  ] {
    hasher.update(value.to_le_bytes());
  }
  hex::encode(hasher.finalize())
}

fn highlight_pixel_rect(
  page: &PageImage,
  search_match: &PdfSearchMatch,
) -> Option<(u32, u32, u32, u32)> {
  let width = page.width.max(1);
  let height = page.height.max(1);
  let (full_width, full_height, slice_x, slice_y) = if let Some(slice) = &page.slice {
    (
      slice.full_pixel_width.max(1),
      slice.full_pixel_height.max(1),
      slice.slice_x,
      slice.slice_y,
    )
  } else {
    (width, height, 0, 0)
  };
  let x_scale = f64::from(full_width.max(1)) / search_match.page_width.max(1.0);
  let y_scale = f64::from(full_height.max(1)) / search_match.page_height.max(1.0);
  let full_x0 = (search_match.rect.x_min * x_scale).floor() as i64 - 2;
  let full_y0 = (search_match.rect.y_min * y_scale).floor() as i64 - 2;
  let full_x1 = (search_match.rect.x_max * x_scale).ceil() as i64 + 2;
  let full_y1 = (search_match.rect.y_max * y_scale).ceil() as i64 + 2;
  let x0 = full_x0 - i64::from(slice_x);
  let y0 = full_y0 - i64::from(slice_y);
  let x1 = full_x1 - i64::from(slice_x);
  let y1 = full_y1 - i64::from(slice_y);
  if x1 <= 0 || y1 <= 0 || x0 >= i64::from(width) || y0 >= i64::from(height) {
    return None;
  }
  let x0 = x0.clamp(0, i64::from(width.saturating_sub(1))) as u32;
  let y0 = y0.clamp(0, i64::from(height.saturating_sub(1))) as u32;
  let x1 = x1.clamp(i64::from(x0.saturating_add(1)), i64::from(width)) as u32;
  let y1 = y1.clamp(i64::from(y0.saturating_add(1)), i64::from(height)) as u32;
  Some((x0, y0, x1, y1))
}
