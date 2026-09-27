use anyhow::Result;
use framework_tui::{SuspendTerminal, TerminalOptions, TerminalOutput, TerminalSession};
use img_tui::{ProtocolFrameOutput, ProtocolFrameRenderer, reset_protocol_images};
use ratatui::Frame;

pub type FrameOutput = ProtocolFrameOutput;

/// Room for a full frame of cell updates, so drawing costs a few writes
/// instead of one per queued command (stderr itself is unbuffered). Every
/// frame and every mode switch ends with an explicit flush.
const OUTPUT_BUFFER_BYTES: usize = 256 * 1024;

/// The terminal session plus the protocol images drawn on it, which are
/// erased before the terminal is handed back and reset when it returns.
pub struct Tui {
  session: TerminalSession,
  protocol_renderer: ProtocolFrameRenderer,
  protocol_reset: Option<String>,
}

impl Tui {
  pub fn new(protocol_reset: Option<String>) -> Result<Self> {
    let mut session = TerminalSession::enter(TerminalOptions {
      output: TerminalOutput::Stderr,
      buffer_capacity: OUTPUT_BUFFER_BYTES,
      ..TerminalOptions::default()
    })?;
    reset_protocol_images(session.backend_mut(), protocol_reset.as_deref())?;
    Ok(Self {
      session,
      protocol_renderer: ProtocolFrameRenderer::default(),
      protocol_reset,
    })
  }

  pub fn draw<F>(&mut self, render: F) -> Result<()>
  where
    F: FnOnce(&mut Frame) -> FrameOutput,
  {
    self
      .protocol_renderer
      .draw(self.session.terminal_mut(), render)
  }

  /// Returns the terminal to its normal state for good. Every step runs
  /// even when an earlier one fails, so a broken image cleanup cannot leave
  /// the shell in raw mode; the first error is reported.
  pub fn restore(&mut self) -> Result<()> {
    let images = self.clear_images();
    let session = self.session.restore();
    images.and(session.map_err(Into::into))
  }

  fn clear_images(&mut self) -> Result<()> {
    if self.session.is_suspended() || self.session.is_restored() {
      return Ok(());
    }
    self
      .protocol_renderer
      .clear_and_reset(self.session.backend_mut(), self.protocol_reset.as_deref())
  }
}

impl SuspendTerminal for Tui {
  type Error = anyhow::Error;

  /// Temporarily hands the terminal to another program.
  fn suspend(&mut self) -> Result<()> {
    let images = self.clear_images();
    let session = self.session.suspend();
    images.and(session.map_err(Into::into))
  }

  fn resume(&mut self) -> Result<()> {
    if !self.session.is_suspended() {
      return Ok(());
    }
    self.session.resume()?;
    reset_protocol_images(self.session.backend_mut(), self.protocol_reset.as_deref())
  }
}

impl Drop for Tui {
  /// Erases the images too when `main` returns early. While unwinding from
  /// a panic the hook has restored the terminal already.
  fn drop(&mut self) {
    if !std::thread::panicking() {
      let _ = self.restore();
    }
  }
}
