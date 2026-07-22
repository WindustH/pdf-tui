use crate::{app::App, pdf::PageImage, search, selection};

pub(super) fn apply_page_overlays(
  app: &App,
  page_index: usize,
  image: &PageImage,
) -> Option<PageImage> {
  let markers = app.selection_markers_for(page_index);
  let outline = app.selection_draft_outline_for(page_index);
  let highlight = app.viewer_search_highlight_for(page_index);
  if markers.is_empty() && outline.is_none() && highlight.is_none() {
    return None;
  }
  let page_size = app.page_dimensions(page_index).unwrap_or((1, 1));
  let mut current = image.clone();
  let mut changed = false;
  if let Some(highlight) = highlight
    && let Ok(Some(highlighted)) = search::highlighted_viewer_image(
      &app.settings.cache_dir,
      &current,
      highlight,
      app.settings.config.render.search_highlight_cache_max_bytes,
    )
  {
    current = highlighted;
    changed = true;
  }
  if let Some(outline) = outline {
    current = selection::outline_page_image(
      &app.settings.cache_dir,
      &current,
      page_size,
      outline,
      app.settings.config.render.selection_cache_max_bytes,
    )
    .ok()?;
    changed = true;
  }
  for marker in markers {
    current = selection::marker_page_image(
      &app.settings.cache_dir,
      &current,
      page_size,
      marker,
      app.settings.config.render.selection_cache_max_bytes,
    )
    .ok()?;
    changed = true;
  }
  changed.then_some(current)
}
