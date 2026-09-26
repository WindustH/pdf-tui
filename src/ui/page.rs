//! Drawing primitives shared by all views: page and slice images, pending
//! and error placeholders, and page frames.

use ratatui::{
  Frame,
  buffer::CellDiffOption,
  layout::{Alignment, Rect},
  style::Style,
  widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::{
  app::App,
  event::RenderedImage,
  geometry::{DEFAULT_CELL_PIXELS, fitted_page_area, page_target_pixels, slot_content_area},
  layout,
  pdf::{PageImage, PageSliceSpec},
  render::RenderKind,
};

use super::{
  DrawCtx,
  page_overlay::{overlaid_or_plain, viewer_overlay_steps},
};

/// Draws one scroll slice; returns whether it is final (rendered at the
/// exact size, or failed) so frame-synced navigation can wait for it.
pub(super) fn draw_slice(
  frame: &mut Frame,
  app: &App,
  ctx: &mut DrawCtx<'_>,
  item: layout::ScrollItem,
  area: Rect,
  viewport: Rect,
) -> bool {
  if area.width == 0 || area.height == 0 {
    return true;
  }
  let spec = slice_spec_for_item(app, item, viewport);
  ctx.pages.request_slice(spec, ctx.tx);
  let slice_label = || {
    format!(
      "page {} slice {}/{}",
      item.page_index + 1,
      item.slice_index + 1,
      item.slice_count
    )
  };

  if let Some(error) = app.slice_error(&spec) {
    draw_centered(frame, area, format!("{} failed\n{error}", slice_label()));
    return true;
  }
  let Some(slice) = app.slice_image(&spec) else {
    draw_image_pending(ctx, area, || format!("rendering {}", slice_label()));
    return false;
  };
  let steps = viewer_overlay_steps(app, item.page_index, slice);
  let (image, marks_ready) = overlaid_or_plain(ctx, slice, &steps);
  let drawn = draw_image(frame, ctx, &image, area, || {
    format!("drawing {}", slice_label())
  });
  drawn && marks_ready
}

/// Page slice request matching a scroll layout item at the current cell
/// size.
pub(super) fn slice_spec_for_item(
  app: &App,
  item: layout::ScrollItem,
  viewport: Rect,
) -> PageSliceSpec {
  let (cell_pixel_width, cell_pixel_height) =
    app.terminal_cell_pixels.unwrap_or(DEFAULT_CELL_PIXELS);
  let target_width =
    u32::from(item.full_width.max(1)).saturating_mul(u32::from(cell_pixel_width.max(1)));
  let target_height =
    u32::from(item.full_height.max(1)).saturating_mul(u32::from(cell_pixel_height.max(1)));
  let slice_count = item.slice_count.max(1);
  let (slice_cell_start, slice_cell_height) = layout::grid_slice_span(
    item.grid_height,
    slice_count,
    item.slice_index,
    item.full_height,
  );
  let slice_y = u64::from(slice_cell_start)
    .saturating_mul(u64::from(cell_pixel_height.max(1)))
    .min(u64::from(u32::MAX)) as u32;
  let slice_height = u64::from(slice_cell_height.max(1))
    .saturating_mul(u64::from(cell_pixel_height.max(1)))
    .max(1)
    .min(u64::from(u32::MAX)) as u32;

  PageSliceSpec {
    page_index: item.page_index,
    slice_index: item.slice_index,
    slice_count,
    target_width,
    target_height,
    slice_y,
    slice_height,
    cell_width: item.width,
    cell_height: item.height,
    full_cell_width: item.full_width,
    full_cell_height: item.full_height,
    grid_cell_height: item.grid_height,
    viewport_width: viewport.width,
    viewport_height: viewport.height,
    scroll_divisor: app.layout.scroll_divisor,
  }
}

/// Where page `index` is drawn inside `area` and the pixel size it is
/// rasterized at.
pub(super) fn fitted_page_request(app: &App, index: usize, area: Rect) -> (Rect, (u32, u32)) {
  let dimensions = app.page_dimensions(index);
  let image_area = fitted_page_area(area, app.terminal_cell_pixels, dimensions);
  let target = page_target_pixels(
    image_area.width,
    image_area.height,
    app.terminal_cell_pixels,
    dimensions,
  );
  (image_area, target)
}

/// Requests page `index` fitted into `area` and draws the rendered page
/// once available. Returns whether the drawn page is final.
pub(super) fn draw_page(
  frame: &mut Frame,
  app: &App,
  ctx: &mut DrawCtx<'_>,
  index: usize,
  area: Rect,
) -> bool {
  if area.width == 0 || area.height == 0 {
    return true;
  }
  let (image_area, (target_width, target_height)) = fitted_page_request(app, index, area);
  if image_area.width == 0 || image_area.height == 0 {
    return true;
  }
  ctx
    .pages
    .request(index, target_width, target_height, ctx.tx);
  let Some(page) = ready_page(frame, app, ctx, index, image_area) else {
    return app.page_error(index).is_some();
  };
  let steps = viewer_overlay_steps(app, index, page);
  let (image, marks_ready) = overlaid_or_plain(ctx, page, &steps);
  let drawn = draw_image(frame, ctx, &image, image_area, || {
    format!("drawing page {}", index + 1)
  });
  drawn && marks_ready
}

/// The rasterized page `index`, or `None` after drawing its error or
/// pending placeholder into `area`.
pub(super) fn ready_page<'a>(
  frame: &mut Frame,
  app: &'a App,
  ctx: &mut DrawCtx<'_>,
  index: usize,
  area: Rect,
) -> Option<&'a PageImage> {
  if let Some(error) = app.page_error(index) {
    draw_centered(frame, area, format!("page {} failed\n{error}", index + 1));
    return None;
  }
  let page = app.page_image(index);
  if page.is_none() {
    draw_image_pending(ctx, area, || format!("rendering page {}", index + 1));
  }
  page
}

/// Draws `image` rendered for the terminal into `area`, or a placeholder
/// while it renders. Returns whether the result is final: the render was
/// drawn or failed.
pub(super) fn draw_image(
  frame: &mut Frame,
  ctx: &mut DrawCtx<'_>,
  image: &PageImage,
  area: Rect,
  pending_message: impl FnOnce() -> String,
) -> bool {
  let cache_key = ctx
    .renderer
    .request(image, area.width, area.height, RenderKind::Fit, ctx.tx);
  if let Some(rendered) = ctx.renderer.get(&cache_key) {
    draw_rendered_image(frame, area, rendered, &mut ctx.images.overlays);
    ctx.images.drawn_render_keys.push(cache_key);
    true
  } else if let Some(error) = ctx.renderer.failure(&cache_key) {
    draw_centered(frame, area, format!("render failed\n{error}"));
    true
  } else {
    draw_image_pending(ctx, area, pending_message);
    false
  }
}

/// Records that `area` is still waiting for an image: protocol images from
/// the previous frame stay in place, and the status line shows the first
/// pending message of the frame.
pub(super) fn draw_image_pending(
  ctx: &mut DrawCtx<'_>,
  area: Rect,
  message: impl FnOnce() -> String,
) {
  if ctx.renderer.draws_with_protocol() {
    ctx.images.preserve_overlays = true;
    ctx.images.preserve_areas.push(area);
  }
  ctx.images.pending_message.get_or_insert_with(message);
}

fn draw_rendered_image(
  frame: &mut Frame,
  area: Rect,
  rendered: &RenderedImage,
  overlays: &mut Vec<img_tui::ProtocolOverlay>,
) {
  match rendered {
    RenderedImage::Symbols { text, .. } => {
      // Render by reference: cloning a full-screen styled `Text` every
      // frame costs one allocation per span.
      frame.render_widget(text, area);
    }
    RenderedImage::Protocol {
      mode,
      data,
      refresh,
      placement,
      fingerprint,
      erase,
    } => {
      reserve_protocol_area(frame, area);
      overlays.push(img_tui::ProtocolOverlay {
        area,
        mode: *mode,
        data: data.clone(),
        refresh: refresh.clone(),
        placement: placement.clone(),
        fingerprint: *fingerprint,
        erase: erase.clone(),
      });
    }
  }
}

/// Draws a grid slot's optional border and returns the area left for the
/// page.
pub(super) fn draw_page_frame(frame: &mut Frame, app: &App, slot: Rect) -> Rect {
  if app.layout.show_border {
    let theme = &app.settings.theme;
    frame.render_widget(
      Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.color(&theme.border))),
      slot,
    );
  }
  slot_content_area(slot, &app.layout)
}

pub(super) fn draw_centered(frame: &mut Frame, area: Rect, text: impl Into<String>) {
  frame.render_widget(
    Paragraph::new(text.into())
      .alignment(Alignment::Center)
      .wrap(Wrap { trim: true }),
    area,
  );
}

/// Marks protocol-image cells so the text diff never overwrites them.
fn reserve_protocol_area(frame: &mut Frame, area: Rect) {
  let area = area.intersection(frame.area());
  let buf = frame.buffer_mut();
  for y in area.top()..area.bottom() {
    for x in area.left()..area.right() {
      if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_diff_option(CellDiffOption::Skip);
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use ratatui::{
    buffer::Buffer,
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::Widget,
  };

  use super::*;

  #[test]
  fn symbols_render_like_the_previous_paragraph_path() {
    let text = Text::from(vec![
      Line::from(vec![
        Span::styled("ab", Style::default().fg(Color::Red)),
        Span::styled("cdefgh", Style::default().bg(Color::Blue)),
      ]),
      Line::from("xyz"),
      Line::from("clipped"),
    ]);
    let area = Rect::new(1, 1, 5, 2);
    let mut expected = Buffer::empty(Rect::new(0, 0, 8, 4));
    Paragraph::new(text.clone()).render(area, &mut expected);
    let mut actual = Buffer::empty(Rect::new(0, 0, 8, 4));
    (&text).render(area, &mut actual);
    assert_eq!(actual, expected);
  }
}
