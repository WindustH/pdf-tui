//! Chooses which marks (search highlight, selection outline and anchors) a
//! page image needs and fetches the marked copy from the overlay store.

use std::borrow::Cow;

use crate::{
  app::App,
  overlay::{OverlayState, OverlayStep},
  pdf::PageImage,
  search,
  selection::{self, PdfRect},
};

use super::DrawCtx;

/// Marks the viewer shows on `image` of page `page_index`, in drawing
/// order; marks outside the image (another scroll slice) are left out.
pub(super) fn viewer_overlay_steps(
  app: &App,
  page_index: usize,
  image: &PageImage,
) -> Vec<OverlayStep> {
  let mut steps = Vec::new();
  if let Some(highlight) = app.viewer_search_highlight_for(page_index)
    && search::highlight_applies(image, highlight)
  {
    steps.push(OverlayStep::SearchHighlight(highlight.clone()));
  }
  let page_size = app.page_dimensions(page_index).unwrap_or((1, 1));
  let visible = |rect: &PdfRect| selection::page_rect_visible(image, page_size, *rect);
  if let Some(rect) = app.selection_draft_outline_for(page_index).filter(visible) {
    steps.push(OverlayStep::PageOutline { page_size, rect });
  }
  for rect in app.selection_markers_for(page_index) {
    if visible(&rect) {
      steps.push(OverlayStep::PageMarker { page_size, rect });
    }
  }
  steps
}

/// The image to draw for `base` with `steps` applied, and whether it is
/// final. Until the marked copy exists the plain image stands in, so the
/// page stays readable without its marks. Fails when marking failed.
pub(super) fn overlaid<'a>(
  ctx: &mut DrawCtx<'_>,
  base: &'a PageImage,
  steps: &[OverlayStep],
) -> Result<(Cow<'a, PageImage>, bool), String> {
  if steps.is_empty() {
    return Ok((Cow::Borrowed(base), true));
  }
  match ctx.overlays.request(base, steps, false, ctx.tx) {
    OverlayState::Ready(image) => Ok((Cow::Owned(image), true)),
    OverlayState::Failed(error) => Err(error),
    OverlayState::Pending => Ok((Cow::Borrowed(base), false)),
  }
}

/// Like [`overlaid`], but a failure falls back to the plain image: the
/// viewer shows the page even when its marks cannot be drawn.
pub(super) fn overlaid_or_plain<'a>(
  ctx: &mut DrawCtx<'_>,
  base: &'a PageImage,
  steps: &[OverlayStep],
) -> (Cow<'a, PageImage>, bool) {
  overlaid(ctx, base, steps).unwrap_or((Cow::Borrowed(base), true))
}
