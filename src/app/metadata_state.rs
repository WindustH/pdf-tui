use crate::metadata;

use super::{App, EditorRequest, ViewMode};

impl App {
  pub(super) fn enter_metadata_view(&mut self) {
    self.view = ViewMode::Metadata;
    self.clear_frame_navigation_lock();
    self.metadata_scroll = 0;
    self.key_dispatcher.clear();
    if let Some(error) = &self.metadata_error {
      self.set_message(format!("metadata unavailable: {error}"));
    } else {
      self.set_message("metadata");
    }
  }

  pub(super) fn start_metadata_edit(&mut self) {
    if self.view != ViewMode::Metadata {
      self.enter_metadata_view();
      return;
    }
    let draft = metadata::metadata_edit_draft(&self.document.path, &self.metadata);
    self.set_editor_request(EditorRequest::Metadata {
      original: self.metadata.clone(),
      draft,
    });
    self.set_message("editing metadata");
  }

  pub(super) fn metadata_scroll_down(&mut self) {
    self.metadata_scroll = self.metadata_scroll.saturating_add(1);
  }

  pub(super) fn metadata_scroll_up(&mut self) {
    self.metadata_scroll = self.metadata_scroll.saturating_sub(1);
  }

  pub(super) fn metadata_page_down(&mut self) {
    self.metadata_scroll = self
      .metadata_scroll
      .saturating_add(self.viewport_height.max(1));
  }

  pub(super) fn metadata_page_up(&mut self) {
    self.metadata_scroll = self
      .metadata_scroll
      .saturating_sub(self.viewport_height.max(1));
  }
}
