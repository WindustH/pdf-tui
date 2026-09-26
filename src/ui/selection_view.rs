use ratatui::{Frame, layout::Rect, widgets::Block, widgets::Paragraph};

use crate::{app::App, geometry::fitted_page_area, overlay::OverlayStep, selection};

use super::{
  DrawCtx, base_style,
  page::{draw_centered, draw_image, draw_image_pending},
  page_overlay::overlaid,
  preload,
};

pub(super) fn draw_selection(frame: &mut Frame, app: &mut App, ctx: &mut DrawCtx<'_>, area: Rect) {
  app.update_viewport(area);
  let ready = draw_selection_preview(frame, app, ctx, area);
  preload::preload_selection_history(app, &mut ctx.preload(), area);
  app.finish_frame_render_pass(ready);
}

fn draw_selection_preview(
  frame: &mut Frame,
  app: &mut App,
  ctx: &mut DrawCtx<'_>,
  area: Rect,
) -> bool {
  app.clear_selection_display();
  if area.width == 0 || area.height == 0 {
    return true;
  }
  frame.render_widget(Block::default().style(base_style(app)), area);
  let Some(selection) = app.current_selection().copied() else {
    frame.render_widget(Paragraph::new("No selection").style(base_style(app)), area);
    return true;
  };
  let selection_index = app.selection.index.unwrap_or(0);
  let (target_width, target_height) = selection::selection_preview_page_target(
    selection,
    area.width,
    area.height,
    app.terminal_cell_pixels,
  );
  let key = app.request_selection_image(selection, target_width, target_height, false, ctx.tx);
  if let Some(error) = app.selection.image_errors.get(&key) {
    draw_centered(frame, area, format!("selection crop failed\n{error}"));
    return true;
  }
  let Some(crop) = app.selection.images.get(&key).cloned() else {
    draw_image_pending(ctx, area, || {
      format!("rendering selection page {}", selection.page_index + 1)
    });
    return false;
  };
  let image_area = fitted_page_area(
    area,
    app.terminal_cell_pixels,
    Some((crop.width.max(1), crop.height.max(1))),
  );
  if image_area.width == 0 || image_area.height == 0 {
    return true;
  }
  app.set_selection_display(selection_index, selection, image_area);
  let mut steps = Vec::new();
  if let Some(rect) = app.selection_draft_outline_for(selection.page_index) {
    steps.push(OverlayStep::CropOutline { selection, rect });
  }
  for rect in app.selection_markers_for(selection.page_index) {
    steps.push(OverlayStep::CropMarker { selection, rect });
  }
  let (display_image, marks_ready) = match overlaid(ctx, &crop, &steps) {
    Ok(overlaid) => overlaid,
    Err(error) => {
      draw_centered(
        frame,
        image_area,
        format!("selection marks failed\n{error}"),
      );
      return true;
    }
  };
  let drawn = draw_image(frame, ctx, &display_image, image_area, || {
    format!("drawing selection page {}", selection.page_index + 1)
  });
  drawn && marks_ready
}
