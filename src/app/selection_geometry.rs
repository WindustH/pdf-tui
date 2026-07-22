use ratatui::layout::{Margin, Rect};

use crate::selection::{PdfPoint, PdfRect, PdfSelection, SelectionAnchor};

use super::{App, SelectionDisplay};

#[derive(Debug, Clone, Copy)]
pub(super) struct SelectionHit {
  pub(super) page_index: usize,
  pub(super) page_width: f64,
  pub(super) page_height: f64,
  pub(super) point: PdfPoint,
  pub(super) cell_rect: PdfRect,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PageDisplay {
  pub(super) page_index: usize,
  pub(super) page_width: f64,
  pub(super) page_height: f64,
  pub(super) area: Rect,
  pub(super) full_cell_width: u16,
  pub(super) full_cell_height: u16,
  pub(super) y_cell_start: u16,
}

pub(super) fn page_display_for_area(
  app: &App,
  page_index: usize,
  area: Rect,
  full_cell_height: u16,
  y_cell_start: u16,
) -> Option<PageDisplay> {
  if area.width == 0 || area.height == 0 {
    return None;
  }
  let (page_width, page_height) = app.page_dimensions(page_index)?;
  Some(PageDisplay {
    page_index,
    page_width: f64::from(page_width.max(1)),
    page_height: f64::from(page_height.max(1)),
    area,
    full_cell_width: area.width.max(1),
    full_cell_height: full_cell_height.max(1),
    y_cell_start,
  })
}

pub(super) fn hit_for_display(
  display: PageDisplay,
  column: u16,
  row: u16,
  clamp_to_display: bool,
) -> SelectionHit {
  let max_column = display
    .area
    .x
    .saturating_add(display.area.width.saturating_sub(1));
  let max_row = display
    .area
    .y
    .saturating_add(display.area.height.saturating_sub(1));
  let column = if clamp_to_display {
    column.clamp(display.area.x, max_column)
  } else {
    column
  };
  let row = if clamp_to_display {
    row.clamp(display.area.y, max_row)
  } else {
    row
  };
  let local_x = column.saturating_sub(display.area.x);
  let local_y = row.saturating_sub(display.area.y);
  let full_y = display.y_cell_start.saturating_add(local_y);
  let x0 = f64::from(local_x) / f64::from(display.full_cell_width.max(1)) * display.page_width;
  let x1 = f64::from(local_x.saturating_add(1)) / f64::from(display.full_cell_width.max(1))
    * display.page_width;
  let y0 = f64::from(full_y) / f64::from(display.full_cell_height.max(1)) * display.page_height;
  let y1 = f64::from(full_y.saturating_add(1)) / f64::from(display.full_cell_height.max(1))
    * display.page_height;
  let raw_cell_rect = PdfRect {
    x_min: x0,
    y_min: y0,
    x_max: x1,
    y_max: y1,
  }
  .normalized();
  let point = clamp_point_to_page(
    PdfPoint {
      x: (raw_cell_rect.x_min + raw_cell_rect.x_max) / 2.0,
      y: (raw_cell_rect.y_min + raw_cell_rect.y_max) / 2.0,
    },
    display.page_width,
    display.page_height,
  );
  let marker = marker_rect_around_point(
    point,
    raw_cell_rect.width().max(f64::EPSILON),
    raw_cell_rect.height().max(f64::EPSILON),
  );
  SelectionHit {
    page_index: display.page_index,
    page_width: display.page_width,
    page_height: display.page_height,
    point,
    cell_rect: marker,
  }
}

pub(super) fn projected_point_for_display(display: PageDisplay, column: u16, row: u16) -> PdfPoint {
  let local_x = i32::from(column) - i32::from(display.area.x);
  let local_y = i32::from(row) - i32::from(display.area.y);
  let full_y = i32::from(display.y_cell_start) + local_y;
  PdfPoint {
    x: (f64::from(local_x) + 0.5) / f64::from(display.full_cell_width.max(1)) * display.page_width,
    y: (f64::from(full_y) + 0.5) / f64::from(display.full_cell_height.max(1)) * display.page_height,
  }
}

pub(super) fn hit_for_selection_display(
  display: &SelectionDisplay,
  column: u16,
  row: u16,
  clamp_to_display: bool,
) -> SelectionHit {
  let max_column = display
    .area
    .x
    .saturating_add(display.area.width.saturating_sub(1));
  let max_row = display
    .area
    .y
    .saturating_add(display.area.height.saturating_sub(1));
  let column = if clamp_to_display {
    column.clamp(display.area.x, max_column)
  } else {
    column
  };
  let row = if clamp_to_display {
    row.clamp(display.area.y, max_row)
  } else {
    row
  };
  let local_x = column.saturating_sub(display.area.x);
  let local_y = row.saturating_sub(display.area.y);
  let area_width = f64::from(display.area.width.max(1));
  let area_height = f64::from(display.area.height.max(1));
  let rect_width = display.rect.width().max(1.0);
  let rect_height = display.rect.height().max(1.0);
  let x0 = display.rect.x_min + f64::from(local_x) / area_width * rect_width;
  let x1 = display.rect.x_min + f64::from(local_x.saturating_add(1)) / area_width * rect_width;
  let y0 = display.rect.y_min + f64::from(local_y) / area_height * rect_height;
  let y1 = display.rect.y_min + f64::from(local_y.saturating_add(1)) / area_height * rect_height;
  let raw_cell_rect = PdfRect {
    x_min: x0,
    y_min: y0,
    x_max: x1,
    y_max: y1,
  }
  .normalized();
  let point = clamp_point_to_page(
    PdfPoint {
      x: (raw_cell_rect.x_min + raw_cell_rect.x_max) / 2.0,
      y: (raw_cell_rect.y_min + raw_cell_rect.y_max) / 2.0,
    },
    display.page_width,
    display.page_height,
  );
  let marker = marker_rect_around_point(
    point,
    raw_cell_rect.width().max(f64::EPSILON),
    raw_cell_rect.height().max(f64::EPSILON),
  );
  SelectionHit {
    page_index: display.page_index,
    page_width: display.page_width,
    page_height: display.page_height,
    point,
    cell_rect: marker,
  }
}

pub(super) fn projected_point_for_selection_display(
  display: &SelectionDisplay,
  column: u16,
  row: u16,
) -> PdfPoint {
  let local_x = i32::from(column) - i32::from(display.area.x);
  let local_y = i32::from(row) - i32::from(display.area.y);
  let rect_width = display.rect.width().max(1.0);
  let rect_height = display.rect.height().max(1.0);
  PdfPoint {
    x: display.rect.x_min
      + (f64::from(local_x) + 0.5) / f64::from(display.area.width.max(1)) * rect_width,
    y: display.rect.y_min
      + (f64::from(local_y) + 0.5) / f64::from(display.area.height.max(1)) * rect_height,
  }
}

pub(super) fn anchor_from_hit(hit: SelectionHit) -> SelectionAnchor {
  SelectionAnchor {
    page_index: hit.page_index,
    page_width: hit.page_width,
    page_height: hit.page_height,
    point: hit.point,
    marker: hit.cell_rect,
  }
}

pub(super) fn anchor_at_point(
  page_index: usize,
  page_width: f64,
  page_height: f64,
  point: PdfPoint,
  marker_width: f64,
  marker_height: f64,
  bounds: Option<PdfRect>,
) -> SelectionAnchor {
  let point = clamp_point_to_page(point, page_width, page_height);
  let point = if let Some(bounds) = bounds {
    PdfPoint {
      x: point.x.clamp(bounds.x_min, bounds.x_max),
      y: point.y.clamp(bounds.y_min, bounds.y_max),
    }
  } else {
    point
  };
  let marker = marker_rect_around_point(point, marker_width.max(1.0), marker_height.max(1.0));
  SelectionAnchor {
    page_index,
    page_width,
    page_height,
    point,
    marker,
  }
}

fn marker_rect_around_point(point: PdfPoint, marker_width: f64, marker_height: f64) -> PdfRect {
  PdfRect {
    x_min: point.x - marker_width / 2.0,
    y_min: point.y - marker_height / 2.0,
    x_max: point.x + marker_width / 2.0,
    y_max: point.y + marker_height / 2.0,
  }
  .normalized()
}

fn clamp_point_to_page(point: PdfPoint, page_width: f64, page_height: f64) -> PdfPoint {
  PdfPoint {
    x: point.x.clamp(0.0, page_width.max(1.0)),
    y: point.y.clamp(0.0, page_height.max(1.0)),
  }
}

pub(super) fn bounded_opposite_point(
  reference: PdfPoint,
  point: PdfPoint,
  anchor: SelectionAnchor,
  bounds: Option<PdfSelection>,
) -> Option<PdfPoint> {
  let bounds = bounds.map(|selection| selection.rect).unwrap_or(PdfRect {
    x_min: 0.0,
    y_min: 0.0,
    x_max: anchor.page_width.max(1.0),
    y_max: anchor.page_height.max(1.0),
  });
  let rect = PdfRect {
    x_min: reference.x.min(point.x),
    y_min: reference.y.min(point.y),
    x_max: reference.x.max(point.x),
    y_max: reference.y.max(point.y),
  };
  let intersected = rect.intersection(bounds)?;
  Some(PdfPoint {
    x: if point.x >= reference.x {
      intersected.x_max
    } else {
      intersected.x_min
    },
    y: if point.y >= reference.y {
      intersected.y_max
    } else {
      intersected.y_min
    },
  })
}

pub(super) fn normalized_distance(
  a: PdfPoint,
  b: PdfPoint,
  page_width: f64,
  page_height: f64,
) -> f64 {
  let dx = (a.x - b.x) / page_width.max(1.0);
  let dy = (a.y - b.y) / page_height.max(1.0);
  dx * dx + dy * dy
}

pub(super) fn contains(area: Rect, column: u16, row: u16) -> bool {
  column >= area.x
    && column < area.x.saturating_add(area.width)
    && row >= area.y
    && row < area.y.saturating_add(area.height)
}

pub(super) fn distance_to_rect(area: Rect, column: u16, row: u16) -> u32 {
  let x0 = i32::from(area.x);
  let y0 = i32::from(area.y);
  let x1 = i32::from(area.x.saturating_add(area.width.saturating_sub(1)));
  let y1 = i32::from(area.y.saturating_add(area.height.saturating_sub(1)));
  let x = i32::from(column).clamp(x0, x1);
  let y = i32::from(row).clamp(y0, y1);
  column
    .abs_diff(u16::try_from(x).unwrap_or_default())
    .pow(2)
    .saturating_add(row.abs_diff(u16::try_from(y).unwrap_or_default()).pow(2))
    .into()
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
  (
    (f64::from(page_width.max(1)) * scale)
      .round()
      .clamp(1.0, f64::from(u32::MAX)) as u32,
    (f64::from(page_height.max(1)) * scale)
      .round()
      .clamp(1.0, f64::from(u32::MAX)) as u32,
  )
}

fn ceil_div_u32(value: u32, divisor: u32) -> u32 {
  value
    .saturating_add(divisor.saturating_sub(1))
    .saturating_div(divisor.max(1))
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
