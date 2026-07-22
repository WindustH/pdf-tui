use crate::selection::PdfSelection;

#[derive(Debug, Clone, Copy)]
pub(super) struct CropPixelRect {
  pub(super) x: u32,
  pub(super) y: u32,
  pub(super) width: u32,
  pub(super) height: u32,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct SelectionCropPlan {
  pub(super) page_width: u32,
  pub(super) page_height: u32,
  pub(super) crop: CropPixelRect,
}

pub(super) fn selection_crop_plan(
  selection: PdfSelection,
  crop_width: u32,
  crop_height: u32,
) -> Option<SelectionCropPlan> {
  let page_width = selection.page_width.max(1.0);
  let page_height = selection.page_height.max(1.0);
  let rect = selection.rect.clamp_to_page(page_width, page_height);
  if rect.is_empty() {
    return None;
  }
  let crop_width = crop_width.max(1);
  let crop_height = crop_height.max(1);
  let x_scale = f64::from(crop_width) / rect.width().max(1.0);
  let y_scale = f64::from(crop_height) / rect.height().max(1.0);
  let scaled_page_width = scaled_dimension(page_width, x_scale);
  let scaled_page_height = scaled_dimension(page_height, y_scale);
  let x0 = (rect.x_min * x_scale).floor() as i64;
  let y0 = (rect.y_min * y_scale).floor() as i64;
  let crop_width = crop_width.min(scaled_page_width).max(1);
  let crop_height = crop_height.min(scaled_page_height).max(1);
  let x = x0.clamp(0, i64::from(scaled_page_width.saturating_sub(crop_width))) as u32;
  let y = y0.clamp(0, i64::from(scaled_page_height.saturating_sub(crop_height))) as u32;
  Some(SelectionCropPlan {
    page_width: scaled_page_width,
    page_height: scaled_page_height,
    crop: CropPixelRect {
      x,
      y,
      width: crop_width,
      height: crop_height,
    },
  })
}

fn scaled_dimension(value: f64, scale: f64) -> u32 {
  (value.max(1.0) * scale.max(0.01))
    .ceil()
    .clamp(1.0, f64::from(u32::MAX)) as u32
}
