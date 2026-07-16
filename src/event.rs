use crossterm::event::Event;
use img_tui::{ProtocolPlacement, RenderMode};
use ratatui::text::Text;

use crate::{
  cache::CacheCleanupReport,
  metadata::PdfMetadataEntry,
  pdf::PdfDocument,
  pdf::{PageImage, PageSliceSpec},
};

#[derive(Debug)]
pub enum AsyncEvent {
  Input { event: Event, generation: u64 },
  Page(PageOutcome),
  Render(RenderOutcome),
  CacheClear(CacheClearOutcome),
  AutoRefreshRequested,
  Refresh(DocumentReloadOutcome),
  MetadataWrite(MetadataWriteOutcome),
}

#[derive(Debug)]
pub struct DocumentReload {
  pub document: PdfDocument,
  pub metadata: Result<Vec<PdfMetadataEntry>, String>,
}

#[derive(Debug)]
pub struct DocumentReloadOutcome {
  pub result: Result<DocumentReload, String>,
}

#[derive(Debug)]
pub struct MetadataWriteOutcome {
  pub result: Result<DocumentReload, String>,
  pub changed_tags: usize,
}

#[derive(Debug)]
pub struct CacheClearOutcome {
  pub result: Result<CacheCleanupReport, String>,
}

#[derive(Debug)]
pub struct PageOutcome {
  pub source_size_bytes: u64,
  pub source_modified_nanos: u128,
  pub page_index: usize,
  pub target_width: u32,
  pub target_height: u32,
  pub slice: Option<PageSliceSpec>,
  pub preload: bool,
  pub result: Result<PageImage, String>,
}

#[derive(Debug)]
pub struct RenderOutcome {
  pub cache_key: String,
  pub slot_key: String,
  pub preload: bool,
  pub result: Result<RenderedImage, String>,
}

#[derive(Debug, Clone)]
pub enum RenderedImage {
  Symbols {
    mode: RenderMode,
    text: Text<'static>,
  },
  Protocol {
    mode: RenderMode,
    data: String,
    refresh: Option<String>,
    placement: Option<ProtocolPlacement>,
    fingerprint: u64,
    erase: Option<String>,
  },
}
