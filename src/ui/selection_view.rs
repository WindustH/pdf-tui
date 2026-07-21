use img_tui::ProtocolOverlay;
use ratatui::{
  Frame,
  layout::Rect,
  style::Style,
  widgets::{Block, Paragraph},
};
use tokio::sync::mpsc;

use crate::{
  app::App,
  event::AsyncEvent,
  pdf::PageStore,
  render::{RenderKind, RenderStore},
  selection,
};

use super::{page, preload};

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_selection(
  frame: &mut Frame,
  app: &mut App,
  _pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  area: Rect,
  obscured_areas: &[Rect],
  overlays: &mut Vec<ProtocolOverlay>,
  frame_message: &mut Option<String>,
  preserve_overlays: &mut bool,
  preserve_areas: &mut Vec<Rect>,
  drawn_render_keys: &mut Vec<String>,
) {
  app.update_viewport(area);
  let ready = draw_selection_preview(
    frame,
    app,
    renderer,
    tx,
    area,
    obscured_areas,
    overlays,
    frame_message,
    preserve_overlays,
    preserve_areas,
    drawn_render_keys,
  );
  preload::preload_selection_history(app, renderer, tx, area);
  app.finish_frame_render_pass(ready);
}

#[allow(clippy::too_many_arguments)]
fn draw_selection_preview(
  frame: &mut Frame,
  app: &mut App,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  area: Rect,
  obscured_areas: &[Rect],
  overlays: &mut Vec<ProtocolOverlay>,
  frame_message: &mut Option<String>,
  preserve_overlays: &mut bool,
  preserve_areas: &mut Vec<Rect>,
  drawn_render_keys: &mut Vec<String>,
) -> bool {
  let theme = &app.settings.theme;
  let base = Style::default()
    .fg(theme.color(&theme.foreground))
    .bg(theme.color(&theme.background));
  if area.width == 0 || area.height == 0 {
    app.clear_selection_display();
    return true;
  }
  frame.render_widget(Block::default().style(base), area);
  let Some(selection) = app.current_selection().copied() else {
    app.clear_selection_display();
    frame.render_widget(Paragraph::new("No selection").style(base), area);
    return true;
  };
  let selection_index = app.selection_index.unwrap_or(0);
  let (target_width, target_height) = selection::selection_preview_page_target(
    selection,
    area.width,
    area.height,
    app.terminal_cell_pixels,
  );
  let key = app.request_selection_image(selection, target_width, target_height, false, tx);
  if let Some(error) = app.selection_image_errors.get(&key).cloned() {
    app.clear_selection_display();
    page::draw_centered(frame, area, format!("selection crop failed\n{error}"));
    return true;
  }
  let Some(crop) = app.selection_images.get(&key).cloned() else {
    app.clear_selection_display();
    page::draw_image_pending(
      frame,
      area,
      renderer,
      format!("rendering selection page {}", selection.page_index + 1),
      frame_message,
      preserve_overlays,
      preserve_areas,
    );
    return false;
  };
  let image_area = page::fitted_page_area(
    area,
    app.terminal_cell_pixels,
    Some((crop.width.max(1), crop.height.max(1))),
  );
  if image_area.width == 0 || image_area.height == 0 {
    app.clear_selection_display();
    return true;
  }
  if page::area_intersects_any(image_area, obscured_areas) {
    app.clear_selection_display();
    return true;
  }
  app.set_selection_display(selection_index, selection, image_area);
  let mut display_image = crop;
  if let Some(outline) = app.selection_draft_outline_for(selection.page_index) {
    display_image = match selection::outline_selection_crop_image(
      &app.settings.cache_dir,
      &display_image,
      selection,
      outline,
      app.settings.config.render.selection_cache_max_bytes,
    ) {
      Ok(outlined) => outlined,
      Err(error) => {
        page::draw_centered(
          frame,
          image_area,
          format!("selection outline failed\n{error}"),
        );
        return true;
      }
    };
  }
  for marker in app.selection_markers_for(selection.page_index) {
    display_image = match selection::marker_selection_crop_image(
      &app.settings.cache_dir,
      &display_image,
      selection,
      marker,
      app.settings.config.render.selection_cache_max_bytes,
    ) {
      Ok(marked) => marked,
      Err(error) => {
        page::draw_centered(
          frame,
          image_area,
          format!("selection marker failed\n{error}"),
        );
        return true;
      }
    }
  }
  let request = renderer.request(
    &display_image,
    image_area.width,
    image_area.height,
    RenderKind::Fit,
    tx,
  );
  if let Some(rendered_key) = renderer.rendered_key(&request.cache_key, &request.slot_key, false) {
    if let Some(rendered) = renderer.get(&rendered_key) {
      page::draw_rendered_page(frame, image_area, rendered, overlays);
      drawn_render_keys.push(rendered_key);
    }
    true
  } else if let Some(error) = renderer.failure(&request.cache_key) {
    page::draw_centered(frame, image_area, format!("render failed\n{error}"));
    true
  } else {
    page::draw_image_pending(
      frame,
      image_area,
      renderer,
      format!("drawing selection page {}", selection.page_index + 1),
      frame_message,
      preserve_overlays,
      preserve_areas,
    );
    false
  }
}
