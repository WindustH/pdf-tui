use img_tui::ProtocolOverlay;
use ratatui::{
  Frame,
  buffer::CellDiffOption,
  layout::{Alignment, Margin, Rect},
  style::Style,
  widgets::{Block, Borders, Paragraph, Wrap},
};
use tokio::sync::mpsc;

use crate::{
  app::App,
  event::{AsyncEvent, RenderedImage},
  layout,
  pdf::{PageSliceSpec, PageStore},
  render::{RenderKind, RenderStore},
};

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_slice(
  frame: &mut Frame,
  app: &App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  item: layout::ScrollItem,
  area: Rect,
  viewport: Rect,
  obscured_areas: &[Rect],
  overlays: &mut Vec<ProtocolOverlay>,
  frame_message: &mut Option<String>,
  preserve_overlays: &mut bool,
  preserve_areas: &mut Vec<Rect>,
  drawn_render_keys: &mut Vec<String>,
) -> bool {
  if area.width == 0 || area.height == 0 {
    return true;
  }
  if area_intersects_any(area, obscured_areas) {
    return true;
  }
  let spec = slice_spec_for_item(app, item, viewport);
  pages.request_slice(spec, tx);

  if let Some(error) = app.slice_errors.get(&spec) {
    draw_centered(
      frame,
      area,
      format!(
        "page {} slice {}/{} failed\n{error}",
        item.page_index + 1,
        item.slice_index + 1,
        item.slice_count
      ),
    );
    return true;
  }

  let Some(slice) = app.slices.get(&spec) else {
    draw_image_pending(
      frame,
      area,
      renderer,
      format!(
        "rendering page {} slice {}/{}",
        item.page_index + 1,
        item.slice_index + 1,
        item.slice_count
      ),
      frame_message,
      preserve_overlays,
      preserve_areas,
    );
    return false;
  };

  let request = renderer.request(slice, area.width, area.height, RenderKind::Fit, tx);
  let exact_ready = renderer
    .rendered_key(&request.cache_key, &request.slot_key, false)
    .is_some();
  let current_failed = renderer.failure(&request.cache_key).is_some();
  if let Some(rendered_key) = renderer.rendered_key(&request.cache_key, &request.slot_key, true) {
    if let Some(rendered) = renderer.get(&rendered_key) {
      draw_rendered_page(frame, area, rendered, overlays);
      drawn_render_keys.push(rendered_key);
    }
    exact_ready || current_failed
  } else if let Some(error) = renderer.failure(&request.cache_key) {
    draw_centered(frame, area, format!("render failed\n{error}"));
    true
  } else {
    draw_image_pending(
      frame,
      area,
      renderer,
      format!(
        "drawing page {} slice {}/{}",
        item.page_index + 1,
        item.slice_index + 1,
        item.slice_count
      ),
      frame_message,
      preserve_overlays,
      preserve_areas,
    );
    false
  }
}

pub(super) fn slice_spec_for_item(
  app: &App,
  item: layout::ScrollItem,
  viewport: Rect,
) -> PageSliceSpec {
  let (cell_pixel_width, cell_pixel_height) = app.terminal_cell_pixels.unwrap_or((8, 16));
  let target_width =
    u32::from(item.full_width.max(1)).saturating_mul(u32::from(cell_pixel_width.max(1)));
  let target_height =
    u32::from(item.full_height.max(1)).saturating_mul(u32::from(cell_pixel_height.max(1)));
  let slice_count = item.slice_count.max(1);
  let slice_cell_start =
    (u64::from(item.full_height) * u64::from(item.slice_index)) / u64::from(slice_count);
  let slice_cell_end = (u64::from(item.full_height)
    * u64::from(item.slice_index.saturating_add(1)))
    / u64::from(slice_count);
  let slice_y = slice_cell_start
    .saturating_mul(u64::from(cell_pixel_height.max(1)))
    .min(u64::from(u32::MAX)) as u32;
  let slice_height = slice_cell_end
    .saturating_sub(slice_cell_start)
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
    viewport_width: viewport.width,
    viewport_height: viewport.height,
    scroll_divisor: app.layout.scroll_divisor,
  }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_page(
  frame: &mut Frame,
  app: &App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  index: usize,
  area: Rect,
  _render_width: u16,
  _render_height: u16,
  kind: RenderKind,
  obscured_areas: &[Rect],
  overlays: &mut Vec<ProtocolOverlay>,
  frame_message: &mut Option<String>,
  preserve_overlays: &mut bool,
  preserve_areas: &mut Vec<Rect>,
  drawn_render_keys: &mut Vec<String>,
) -> bool {
  if area.width == 0 || area.height == 0 {
    return true;
  }
  let image_area = fitted_page_area(area, app.terminal_cell_pixels, app.page_dimensions(index));
  if image_area.width == 0 || image_area.height == 0 {
    return true;
  }
  if area_intersects_any(image_area, obscured_areas) {
    return true;
  }
  let (target_width, target_height) = page_target_pixels(
    image_area.width,
    image_area.height,
    app.terminal_cell_pixels,
    app.page_dimensions(index),
  );
  pages.request(index, target_width, target_height, tx);

  if let Some(error) = app.page_errors.get(index).and_then(|error| error.as_ref()) {
    draw_centered(
      frame,
      image_area,
      format!("page {} failed\n{error}", index + 1),
    );
    return true;
  }

  let Some(page) = app.pages.get(index).and_then(|page| page.as_ref()) else {
    draw_image_pending(
      frame,
      image_area,
      renderer,
      format!("rendering page {}", index + 1),
      frame_message,
      preserve_overlays,
      preserve_areas,
    );
    return false;
  };

  let request = renderer.request(page, image_area.width, image_area.height, kind, tx);
  let exact_ready = renderer
    .rendered_key(&request.cache_key, &request.slot_key, false)
    .is_some();
  let current_failed = renderer.failure(&request.cache_key).is_some();
  if let Some(rendered_key) = renderer.rendered_key(&request.cache_key, &request.slot_key, true) {
    if let Some(rendered) = renderer.get(&rendered_key) {
      draw_rendered_page(frame, image_area, rendered, overlays);
      drawn_render_keys.push(rendered_key);
    }
    exact_ready || current_failed
  } else if let Some(error) = renderer.failure(&request.cache_key) {
    draw_centered(frame, image_area, format!("render failed\n{error}"));
    true
  } else {
    draw_image_pending(
      frame,
      image_area,
      renderer,
      format!("drawing page {}", index + 1),
      frame_message,
      preserve_overlays,
      preserve_areas,
    );
    false
  }
}

pub(super) fn draw_image_pending(
  _frame: &mut Frame,
  area: Rect,
  renderer: &RenderStore,
  text: String,
  frame_message: &mut Option<String>,
  preserve_overlays: &mut bool,
  preserve_areas: &mut Vec<Rect>,
) {
  if renderer.draws_with_protocol() {
    *preserve_overlays = true;
    preserve_areas.push(area);
  }
  frame_message.get_or_insert(text);
}

pub(super) fn draw_rendered_page(
  frame: &mut Frame,
  area: Rect,
  rendered: &RenderedImage,
  overlays: &mut Vec<ProtocolOverlay>,
) {
  match rendered {
    RenderedImage::Symbols { mode, text } => {
      let _mode_label = mode.label();
      frame.render_widget(Paragraph::new(text.clone()), area);
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
      overlays.push(ProtocolOverlay {
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

pub(super) fn draw_page_frame(frame: &mut Frame, app: &App, area: Rect, focused: bool) -> Rect {
  if !app.layout.show_border {
    return safe_inner(area, app.layout.padding, app.layout.padding);
  }
  let theme = &app.settings.theme;
  let border = if focused {
    theme.color(&theme.focused_border)
  } else {
    theme.color(&theme.border)
  };
  frame.render_widget(
    Block::default()
      .borders(Borders::ALL)
      .border_style(Style::default().fg(border)),
    area,
  );
  safe_inner(
    area,
    app.layout.padding.saturating_add(1),
    app.layout.padding.saturating_add(1),
  )
}

pub(super) fn page_target_pixels(
  width: u16,
  height: u16,
  cell_pixels: Option<(u16, u16)>,
  page_dimensions: Option<(u32, u32)>,
) -> (u32, u32) {
  let (cell_width, cell_height) = cell_pixels.unwrap_or((8, 16));
  let max_width = u32::from(width.max(1)).saturating_mul(u32::from(cell_width.max(1)));
  let max_height = u32::from(height.max(1)).saturating_mul(u32::from(cell_height.max(1)));
  let Some((page_width, page_height)) = page_dimensions else {
    return (max_width.max(1), max_height.max(1));
  };
  let scale = (f64::from(max_width.max(1)) / f64::from(page_width.max(1)))
    .min(f64::from(max_height.max(1)) / f64::from(page_height.max(1)));
  let target_width = (f64::from(page_width.max(1)) * scale)
    .round()
    .clamp(1.0, f64::from(u32::MAX)) as u32;
  let target_height = (f64::from(page_height.max(1)) * scale)
    .round()
    .clamp(1.0, f64::from(u32::MAX)) as u32;
  (target_width, target_height)
}

pub(super) fn fitted_page_area(
  area: Rect,
  cell_pixels: Option<(u16, u16)>,
  page_dimensions: Option<(u32, u32)>,
) -> Rect {
  if area.width == 0 || area.height == 0 {
    return area;
  }
  let (target_width, target_height) =
    page_target_pixels(area.width, area.height, cell_pixels, page_dimensions);
  let (cell_width, cell_height) = cell_pixels.unwrap_or((8, 16));
  let width = ceil_div_u32(target_width.max(1), u32::from(cell_width.max(1)))
    .min(u32::from(area.width))
    .max(1) as u16;
  let height = ceil_div_u32(target_height.max(1), u32::from(cell_height.max(1)))
    .min(u32::from(area.height))
    .max(1) as u16;
  Rect::new(
    area.x.saturating_add(area.width.saturating_sub(width) / 2),
    area
      .y
      .saturating_add(area.height.saturating_sub(height) / 2),
    width,
    height,
  )
}

fn ceil_div_u32(value: u32, divisor: u32) -> u32 {
  value
    .saturating_add(divisor.saturating_sub(1))
    .saturating_div(divisor.max(1))
}

pub(super) fn draw_centered(frame: &mut Frame, area: Rect, text: impl Into<String>) {
  frame.render_widget(
    Paragraph::new(text.into())
      .alignment(Alignment::Center)
      .wrap(Wrap { trim: true }),
    area,
  );
}

fn reserve_protocol_area(frame: &mut Frame, area: Rect) {
  let buf = frame.buffer_mut();
  for y in area.y..area.y.saturating_add(area.height) {
    for x in area.x..area.x.saturating_add(area.width) {
      if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_diff_option(CellDiffOption::Skip);
      }
    }
  }
}

pub(super) fn safe_inner(area: Rect, horizontal: u16, vertical: u16) -> Rect {
  if area.width <= horizontal.saturating_mul(2) || area.height <= vertical.saturating_mul(2) {
    return Rect::new(area.x, area.y, 0, 0);
  }
  area.inner(Margin {
    horizontal,
    vertical,
  })
}

pub(super) fn area_intersects_any(area: Rect, others: &[Rect]) -> bool {
  others
    .iter()
    .copied()
    .any(|other| rects_intersect(area, other))
}

fn rects_intersect(a: Rect, b: Rect) -> bool {
  a.x < b.x.saturating_add(b.width)
    && b.x < a.x.saturating_add(a.width)
    && a.y < b.y.saturating_add(b.height)
    && b.y < a.y.saturating_add(a.height)
}
