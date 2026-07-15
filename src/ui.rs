use framework_tui::{
  CompletionListStyle, KeyHintsStyle, PromptLineStyle, completion_rows,
  default_completion_selected_style, draw_completion_list, draw_key_hints, draw_prompt_line,
  key_hint_columns, key_hint_rows,
};
use img_tui::ProtocolOverlay;
use ratatui::{
  Frame,
  buffer::CellDiffOption,
  layout::{Alignment, Constraint, Direction, Margin, Rect},
  style::{Modifier, Style},
  text::{Line, Span},
  widgets::{Block, Borders, Paragraph, Wrap},
};
use tokio::sync::mpsc;
use tracing::debug;

use crate::{
  app::App,
  event::{AsyncEvent, RenderedImage},
  layout,
  pdf::{PageSliceSpec, PageStore},
  render::{RenderKind, RenderStore},
  terminal::FrameOutput,
};

pub fn draw(
  frame: &mut Frame,
  app: &mut App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
) -> FrameOutput {
  let area = frame.area();
  let footer_height = footer_height(app, area.width).min(area.height);
  let chunks = ratatui::layout::Layout::default()
    .direction(Direction::Vertical)
    .constraints([Constraint::Min(1), Constraint::Length(footer_height)])
    .split(area);
  let main = chunks[0];
  let footer = chunks[1];
  let mut overlays = Vec::new();
  let mut cursor_position = None;
  let mut frame_message = None;
  let mut preserve_overlays = false;
  let mut preserve_areas = Vec::new();
  let mut drawn_render_keys = Vec::new();

  draw_main(
    frame,
    app,
    pages,
    renderer,
    tx,
    main,
    &mut overlays,
    &mut frame_message,
    &mut preserve_overlays,
    &mut preserve_areas,
    &mut drawn_render_keys,
  );
  draw_footer(
    frame,
    app,
    footer,
    &mut cursor_position,
    frame_message.as_deref(),
  );
  let protocol_writes = if preserve_overlays {
    renderer.take_protocol_writes(&drawn_render_keys, false)
  } else {
    for key in &drawn_render_keys {
      renderer.mark_drawn(key);
    }
    renderer.take_protocol_writes(&drawn_render_keys, true)
  };
  let protocol_write_bytes = protocol_writes.iter().map(String::len).sum::<usize>();
  debug!(
    width = area.width,
    height = area.height,
    main_width = main.width,
    main_height = main.height,
    footer_height,
    overlays = overlays.len(),
    protocol_writes = protocol_writes.len(),
    protocol_write_bytes,
    preserve_overlays,
    "frame output built"
  );

  FrameOutput {
    overlays,
    protocol_writes,
    cursor_position,
    preserve_overlays,
    preserve_areas: if preserve_overlays {
      preserve_areas
    } else {
      Vec::new()
    },
  }
}

fn draw_main(
  frame: &mut Frame,
  app: &mut App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  area: Rect,
  overlays: &mut Vec<ProtocolOverlay>,
  frame_message: &mut Option<String>,
  preserve_overlays: &mut bool,
  preserve_areas: &mut Vec<Rect>,
  drawn_render_keys: &mut Vec<String>,
) {
  let theme = &app.settings.theme;
  frame.render_widget(
    Block::default().style(
      Style::default()
        .fg(theme.color(&theme.foreground))
        .bg(theme.color(&theme.background)),
    ),
    area,
  );

  if app.document.page_count == 0 {
    frame.render_widget(Paragraph::new("No pages"), area);
    return;
  }

  if app.layout.is_scroll() {
    draw_scroll(
      frame,
      app,
      pages,
      renderer,
      tx,
      area,
      overlays,
      frame_message,
      preserve_overlays,
      preserve_areas,
      drawn_render_keys,
    );
  } else {
    draw_grid(
      frame,
      app,
      pages,
      renderer,
      tx,
      area,
      overlays,
      frame_message,
      preserve_overlays,
      preserve_areas,
      drawn_render_keys,
    );
  }
}

fn draw_scroll(
  frame: &mut Frame,
  app: &mut App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  area: Rect,
  overlays: &mut Vec<ProtocolOverlay>,
  frame_message: &mut Option<String>,
  preserve_overlays: &mut bool,
  preserve_areas: &mut Vec<Rect>,
  drawn_render_keys: &mut Vec<String>,
) {
  let scroll_layout = layout::compute_scroll_layout(
    app.document.page_count,
    area.width,
    area.height,
    &app.layout,
    |index| app.page_dimensions(index),
    app.terminal_cell_pixels,
  );
  app.update_scroll_layout(scroll_layout.clone(), area);

  let visible_rows = layout::visible_scroll_rows(
    &scroll_layout,
    app.scroll as usize,
    area.height,
    app.layout.scroll_divisor,
  );
  let used_height = layout::visible_rows_height(&scroll_layout, &visible_rows);
  let mut row_y = area
    .y
    .saturating_add(area.height.saturating_sub(used_height) / 2);
  let mut visible_summary = Vec::new();
  for (row_position, row_index) in visible_rows.iter().copied().enumerate() {
    let Some(row) = scroll_layout.rows.get(row_index) else {
      continue;
    };
    for item_index in &row.items {
      let Some(item) = scroll_layout.items.get(*item_index).copied() else {
        continue;
      };
      let item_y = row_y.saturating_add(row.height.saturating_sub(item.height) / 2);
      let item_area = Rect::new(
        area.x.saturating_add(item.x),
        item_y,
        item.width.min(area.width.saturating_sub(item.x)),
        item.height,
      );
      let ready = draw_slice(
        frame,
        app,
        pages,
        renderer,
        tx,
        item,
        item_area,
        area,
        overlays,
        frame_message,
        preserve_overlays,
        preserve_areas,
        drawn_render_keys,
      );
      visible_summary.push(format!(
        "p{} s{}/{} row={} y={} h={} ready={}",
        item.page_index + 1,
        item.slice_index + 1,
        item.slice_count,
        row_index,
        item_area.y,
        item_area.height,
        ready
      ));
    }
    row_y = row_y.saturating_add(row.height);
    if row_position + 1 < visible_rows.len() {
      row_y = row_y.saturating_add(row.gap_after);
    }
  }
  preload_scroll_neighbors(
    app,
    pages,
    renderer,
    tx,
    area,
    &scroll_layout,
    &visible_rows,
  );
  debug!(
    scroll = app.scroll,
    focused_page = app.focused_page + 1,
    viewport_width = area.width,
    viewport_height = area.height,
    total_height = scroll_layout.total_height,
    rows = scroll_layout.rows.len(),
    visible_rows = ?visible_rows,
    visible = ?visible_summary,
    preserve_overlays = *preserve_overlays,
    "scroll draw"
  );
}

#[allow(clippy::too_many_arguments)]
fn draw_slice(
  frame: &mut Frame,
  app: &App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  item: layout::ScrollItem,
  area: Rect,
  viewport: Rect,
  overlays: &mut Vec<ProtocolOverlay>,
  frame_message: &mut Option<String>,
  preserve_overlays: &mut bool,
  preserve_areas: &mut Vec<Rect>,
  drawn_render_keys: &mut Vec<String>,
) -> bool {
  if area.width == 0 || area.height == 0 {
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
    draw_page_pending(
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
  if let Some(rendered_key) = renderer.rendered_key(&request.cache_key, &request.slot_key, true) {
    if let Some(rendered) = renderer.get(&rendered_key) {
      draw_rendered_page(frame, area, rendered, overlays);
      drawn_render_keys.push(rendered_key);
    }
    true
  } else if let Some(error) = renderer.failure(&request.cache_key) {
    draw_centered(frame, area, format!("render failed\n{error}"));
    true
  } else {
    draw_page_pending(
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

fn slice_spec_for_item(app: &App, item: layout::ScrollItem, viewport: Rect) -> PageSliceSpec {
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

fn draw_grid(
  frame: &mut Frame,
  app: &mut App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  area: Rect,
  overlays: &mut Vec<ProtocolOverlay>,
  frame_message: &mut Option<String>,
  preserve_overlays: &mut bool,
  preserve_areas: &mut Vec<Rect>,
  drawn_render_keys: &mut Vec<String>,
) {
  let slots = layout::grid_slots(area, &app.layout);
  let capacity = slots.len().max(1);
  app.set_grid_viewport(area, capacity);
  let start = app.grid_start_page;
  let mut visible = Vec::new();

  for (slot_index, slot) in slots.into_iter().enumerate() {
    let page_index = start + slot_index;
    if page_index >= app.document.page_count {
      continue;
    }
    visible.push(page_index);
    let page_area = draw_page_frame(frame, app, slot, false);
    draw_page(
      frame,
      app,
      pages,
      renderer,
      tx,
      page_index,
      page_area,
      page_area.width,
      page_area.height,
      RenderKind::Fit,
      overlays,
      frame_message,
      preserve_overlays,
      preserve_areas,
      drawn_render_keys,
    );
  }
  preload_grid_neighbors(app, pages, renderer, tx, area, &visible);
}

#[allow(clippy::too_many_arguments)]
fn draw_page(
  frame: &mut Frame,
  app: &App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  index: usize,
  area: Rect,
  render_width: u16,
  render_height: u16,
  kind: RenderKind,
  overlays: &mut Vec<ProtocolOverlay>,
  frame_message: &mut Option<String>,
  preserve_overlays: &mut bool,
  preserve_areas: &mut Vec<Rect>,
  drawn_render_keys: &mut Vec<String>,
) -> bool {
  if area.width == 0 || area.height == 0 {
    return true;
  }
  let (target_width, target_height) = page_target_pixels(
    render_width,
    render_height,
    app.terminal_cell_pixels,
    app.page_dimensions(index),
  );
  pages.request(index, target_width, target_height, tx);

  if let Some(error) = app.page_errors.get(index).and_then(|error| error.as_ref()) {
    draw_centered(frame, area, format!("page {} failed\n{error}", index + 1));
    return true;
  }

  let Some(page) = app.pages.get(index).and_then(|page| page.as_ref()) else {
    draw_page_pending(
      frame,
      area,
      renderer,
      format!("rendering page {}", index + 1),
      frame_message,
      preserve_overlays,
      preserve_areas,
    );
    return false;
  };

  let request = renderer.request(page, render_width, render_height, kind, tx);
  if let Some(rendered_key) = renderer.rendered_key(&request.cache_key, &request.slot_key, true) {
    if let Some(rendered) = renderer.get(&rendered_key) {
      draw_rendered_page(frame, area, rendered, overlays);
      drawn_render_keys.push(rendered_key);
    }
    true
  } else if let Some(error) = renderer.failure(&request.cache_key) {
    draw_centered(frame, area, format!("render failed\n{error}"));
    true
  } else {
    draw_page_pending(
      frame,
      area,
      renderer,
      format!("drawing page {}", index + 1),
      frame_message,
      preserve_overlays,
      preserve_areas,
    );
    false
  }
}

fn draw_page_pending(
  frame: &mut Frame,
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
    frame_message.get_or_insert(text);
    return;
  }
  draw_centered(frame, area, text);
}

fn draw_rendered_page(
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

fn draw_page_frame(frame: &mut Frame, app: &App, area: Rect, focused: bool) -> Rect {
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

fn preload_scroll_neighbors(
  app: &App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  area: Rect,
  scroll_layout: &layout::ScrollLayout,
  visible_rows: &[usize],
) {
  if visible_rows.is_empty() {
    return;
  }
  let first = visible_rows[0];
  let last = *visible_rows.last().unwrap_or(&first);
  let ahead = app.settings.config.render.preload_ahead;
  let behind = app.settings.config.render.preload_behind;
  for index in first.saturating_sub(behind)..first {
    preload_scroll_row(app, pages, renderer, tx, area, scroll_layout, index);
  }
  for index in last.saturating_add(1)..=last.saturating_add(ahead) {
    if index >= scroll_layout.rows.len() {
      break;
    }
    preload_scroll_row(app, pages, renderer, tx, area, scroll_layout, index);
  }
}

fn preload_scroll_row(
  app: &App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  area: Rect,
  scroll_layout: &layout::ScrollLayout,
  row_index: usize,
) {
  let Some(row) = scroll_layout.rows.get(row_index) else {
    return;
  };
  for item_index in &row.items {
    let Some(item) = scroll_layout.items.get(*item_index).copied() else {
      continue;
    };
    let spec = slice_spec_for_item(app, item, area);
    pages.preload_slice(spec, tx);
    if let Some(slice) = app.slices.get(&spec) {
      renderer.preload(slice, item.width, item.height, RenderKind::Fit, tx);
    }
  }
}

fn preload_grid_neighbors(
  app: &App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  area: Rect,
  visible: &[usize],
) {
  if visible.is_empty() {
    return;
  }
  let first = visible[0];
  let last = *visible.last().unwrap_or(&first);
  let ahead = app.settings.config.render.preload_ahead;
  let behind = app.settings.config.render.preload_behind;
  let slots = layout::grid_slots(area, &app.layout);
  let Some(slot) = slots.first().copied() else {
    return;
  };
  let page_area = if app.layout.show_border {
    safe_inner(
      slot,
      app.layout.padding.saturating_add(1),
      app.layout.padding.saturating_add(1),
    )
  } else {
    safe_inner(slot, app.layout.padding, app.layout.padding)
  };
  if page_area.width == 0 || page_area.height == 0 {
    return;
  }

  for index in first.saturating_sub(behind)..first {
    preload_grid_page(app, pages, renderer, tx, index, page_area);
  }
  for index in last.saturating_add(1)..=last.saturating_add(ahead) {
    if index >= app.document.page_count {
      break;
    }
    preload_grid_page(app, pages, renderer, tx, index, page_area);
  }
}

fn preload_grid_page(
  app: &App,
  pages: &mut PageStore,
  renderer: &mut RenderStore,
  tx: &mpsc::UnboundedSender<AsyncEvent>,
  index: usize,
  page_area: Rect,
) {
  let (target_width, target_height) = page_target_pixels(
    page_area.width,
    page_area.height,
    app.terminal_cell_pixels,
    app.page_dimensions(index),
  );
  pages.preload(index, target_width, target_height, tx);
  if let Some(page) = app.pages.get(index).and_then(|page| page.as_ref()) {
    renderer.preload(page, page_area.width, page_area.height, RenderKind::Fit, tx);
  }
}

fn page_target_pixels(
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

fn draw_centered(frame: &mut Frame, area: Rect, text: impl Into<String>) {
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

fn safe_inner(area: Rect, horizontal: u16, vertical: u16) -> Rect {
  if area.width <= horizontal.saturating_mul(2) || area.height <= vertical.saturating_mul(2) {
    return Rect::new(area.x, area.y, 0, 0);
  }
  area.inner(Margin {
    horizontal,
    vertical,
  })
}

fn footer_height(app: &App, width: u16) -> u16 {
  let status = 1_u16;
  let prompt = u16::from(app.prompt.is_some());
  let completion = if app.prompt.is_some() {
    completion_rows(app.command_completion(), 5)
  } else {
    0
  };
  let hints = app.key_hints();
  let which = if hints.is_empty() {
    0
  } else {
    key_hint_rows(hints.len(), which_key_columns(app, width))
  };
  status
    .saturating_add(prompt)
    .saturating_add(completion)
    .saturating_add(which)
}

fn draw_footer(
  frame: &mut Frame,
  app: &App,
  area: Rect,
  cursor_position: &mut Option<(u16, u16)>,
  frame_message: Option<&str>,
) {
  if area.height == 0 {
    return;
  }
  let theme = &app.settings.theme;
  frame.render_widget(
    Block::default().style(
      Style::default()
        .fg(theme.color(&theme.foreground))
        .bg(theme.color(&theme.background)),
    ),
    area,
  );
  let status_area = Rect::new(
    area.x,
    area.y + area.height.saturating_sub(1),
    area.width,
    1,
  );
  let mut content_bottom = area.y.saturating_add(area.height.saturating_sub(1));

  if let Some(prompt) = &app.prompt
    && content_bottom > area.y
  {
    content_bottom = content_bottom.saturating_sub(1);
    let prompt_area = Rect::new(area.x, content_bottom, area.width, 1);
    draw_prompt(frame, app, prompt, prompt_area, cursor_position);
  }

  let completion_rows = command_completion_rows(app);
  if completion_rows > 0 && content_bottom > area.y {
    let height = completion_rows.min(content_bottom - area.y);
    content_bottom = content_bottom.saturating_sub(height);
    let completion_area = Rect::new(area.x, content_bottom, area.width, height);
    draw_command_completion(frame, app, completion_area);
  }

  if !app.key_hints().is_empty() && area.y < content_bottom {
    let which_area = Rect::new(area.x, area.y, area.width, content_bottom - area.y);
    draw_which_key(frame, app, which_area);
  }

  draw_status(frame, app, status_area, frame_message);
}

fn command_completion_rows(app: &App) -> u16 {
  if app.prompt.is_some() {
    completion_rows(app.command_completion(), 5)
  } else {
    0
  }
}

fn which_key_columns(app: &App, width: u16) -> usize {
  key_hint_columns(app.settings.theme.which_key_columns as usize, width)
}

fn draw_prompt(
  frame: &mut Frame,
  app: &App,
  prompt: &framework_tui::Prompt,
  area: Rect,
  cursor_position: &mut Option<(u16, u16)>,
) {
  let theme = &app.settings.theme;
  let base = Style::default()
    .fg(theme.color(&theme.foreground))
    .bg(theme.color(&theme.background));
  let style = PromptLineStyle {
    base,
    prefix: base.fg(theme.color(&theme.accent)),
    suggestion: base.fg(theme.color(&theme.muted)),
  };
  if let Some(position) = draw_prompt_line(frame, prompt, app.command_completion(), area, &style) {
    *cursor_position = Some(position);
  }
}

fn draw_command_completion(frame: &mut Frame, app: &App, area: Rect) {
  let Some(completion) = app.command_completion() else {
    return;
  };
  let theme = &app.settings.theme;
  let base = Style::default()
    .fg(theme.color(&theme.which_key_foreground))
    .bg(theme.color(&theme.which_key_background));
  let style = CompletionListStyle {
    base,
    selected: default_completion_selected_style(),
  };
  draw_completion_list(frame, completion, area, &style);
}

fn draw_which_key(frame: &mut Frame, app: &App, area: Rect) {
  let theme = &app.settings.theme;
  let base = Style::default()
    .fg(theme.color(&theme.which_key_foreground))
    .bg(theme.color(&theme.which_key_background));
  let style = KeyHintsStyle {
    base,
    key: base
      .fg(theme.color(&theme.which_key_key))
      .add_modifier(Modifier::BOLD),
    separator: base.fg(theme.color(&theme.which_key_separator_color)),
    description: base.fg(theme.color(&theme.which_key_description)),
    separator_text: theme.which_key_separator.clone(),
    columns: theme.which_key_columns as usize,
  };
  draw_key_hints(frame, app.key_hints(), area, &style);
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect, frame_message: Option<&str>) {
  let theme = &app.settings.theme;
  let style = Style::default()
    .fg(theme.color(&theme.foreground))
    .bg(theme.color(&theme.background));
  let page = status_page_label(app);
  frame.render_widget(
    Paragraph::new(Line::from(vec![
      Span::styled("pdf", style.fg(theme.color(&theme.accent))),
      Span::styled(
        format!(
          "  {page}/{}  {}  {}  {}",
          app.document.page_count,
          app.layout.label(),
          app.document.file_name,
          frame_message.unwrap_or(&app.message)
        ),
        style,
      ),
    ]))
    .style(style),
    area,
  );
}

fn status_page_label(app: &App) -> String {
  if app.document.page_count == 0 {
    return "0".to_string();
  }
  if app.layout.is_scroll() {
    return (app.focused_page + 1).to_string();
  }
  let start = app
    .grid_start_page
    .min(app.document.page_count.saturating_sub(1));
  let end = start
    .saturating_add(app.layout.grid_capacity().max(1))
    .min(app.document.page_count);
  if end <= start + 1 {
    (start + 1).to_string()
  } else {
    format!("{}-{}", start + 1, end)
  }
}
