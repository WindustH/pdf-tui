//! Cell and pixel geometry shared by drawing, preloading, and mouse hit
//! testing, so all three agree on where a page is shown.

use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};

use crate::config::EffectiveLayoutConfig;

/// Terminal cell size assumed when the terminal does not report one.
pub const DEFAULT_CELL_PIXELS: (u16, u16) = (8, 16);

pub fn contains(area: Rect, column: u16, row: u16) -> bool {
  column >= area.x
    && column < area.x.saturating_add(area.width)
    && row >= area.y
    && row < area.y.saturating_add(area.height)
}

/// `area` shrunk by the given margins, or an empty rect when nothing is left.
pub fn safe_inner(area: Rect, horizontal: u16, vertical: u16) -> Rect {
  if area.width <= horizontal.saturating_mul(2) || area.height <= vertical.saturating_mul(2) {
    return Rect::new(area.x, area.y, 0, 0);
  }
  area.inner(Margin {
    horizontal,
    vertical,
  })
}

/// Area inside a grid slot once the optional page border and padding are
/// taken away.
pub fn slot_content_area(slot: Rect, layout: &EffectiveLayoutConfig) -> Rect {
  let inset = layout.padding.saturating_add(u16::from(layout.show_border));
  safe_inner(slot, inset, inset)
}

/// Splits `area` horizontally into the list and preview panels of the
/// bookmarks and search views.
pub fn split_panels(area: Rect, left_ratio: u16, right_ratio: u16) -> (Rect, Rect) {
  let left = u32::from(left_ratio.max(1));
  let right = u32::from(right_ratio.max(1));
  let chunks = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([
      Constraint::Ratio(left, left + right),
      Constraint::Ratio(right, left + right),
    ])
    .split(area);
  (chunks[0], chunks[1])
}

/// Pixel size of a page scaled to fit `width` x `height` cells while keeping
/// its aspect ratio.
pub fn page_target_pixels(
  width: u16,
  height: u16,
  cell_pixels: Option<(u16, u16)>,
  page_dimensions: Option<(u32, u32)>,
) -> (u32, u32) {
  let (cell_width, cell_height) = cell_pixels.unwrap_or(DEFAULT_CELL_PIXELS);
  let max_width = u32::from(width.max(1)).saturating_mul(u32::from(cell_width.max(1)));
  let max_height = u32::from(height.max(1)).saturating_mul(u32::from(cell_height.max(1)));
  let Some((page_width, page_height)) = page_dimensions else {
    return (max_width.max(1), max_height.max(1));
  };
  let scale = (f64::from(max_width.max(1)) / f64::from(page_width.max(1)))
    .min(f64::from(max_height.max(1)) / f64::from(page_height.max(1)));
  let scaled = |value: u32| {
    (f64::from(value.max(1)) * scale)
      .round()
      .clamp(1.0, f64::from(u32::MAX)) as u32
  };
  (scaled(page_width), scaled(page_height))
}

/// The cells a page occupies when fitted and centered inside `area`.
pub fn fitted_page_area(
  area: Rect,
  cell_pixels: Option<(u16, u16)>,
  page_dimensions: Option<(u32, u32)>,
) -> Rect {
  if area.width == 0 || area.height == 0 {
    return area;
  }
  let (target_width, target_height) =
    page_target_pixels(area.width, area.height, cell_pixels, page_dimensions);
  let (cell_width, cell_height) = cell_pixels.unwrap_or(DEFAULT_CELL_PIXELS);
  let width = target_width
    .div_ceil(u32::from(cell_width.max(1)))
    .clamp(1, u32::from(area.width)) as u16;
  let height = target_height
    .div_ceil(u32::from(cell_height.max(1)))
    .clamp(1, u32::from(area.height)) as u16;
  Rect::new(
    area.x.saturating_add(area.width.saturating_sub(width) / 2),
    area
      .y
      .saturating_add(area.height.saturating_sub(height) / 2),
    width,
    height,
  )
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn fitted_page_area_keeps_aspect_and_centers() {
    // A portrait page in a wide area is limited by height and centered.
    let area = Rect::new(10, 5, 100, 20);
    let fitted = fitted_page_area(area, Some((8, 16)), Some((595, 842)));
    assert_eq!(fitted.height, 20);
    assert!(fitted.width < 100);
    assert_eq!(fitted.x, 10 + (100 - fitted.width) / 2);
    // Degenerate areas never produce an empty page rect or overflow.
    let tiny = fitted_page_area(Rect::new(0, 0, 1, 1), None, Some((1, 100_000)));
    assert_eq!((tiny.width, tiny.height), (1, 1));
  }
}
