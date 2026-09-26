use std::{
  io::{self, BufWriter, Stderr},
  mem::ManuallyDrop,
  thread,
};

use anyhow::Result;
use crossterm::{
  cursor::Show,
  event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture},
  execute,
  terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use img_tui::{ProtocolFrameOutput, ProtocolFrameRenderer, reset_protocol_images};
use ratatui::{Frame, Terminal, prelude::CrosstermBackend};

pub type FrameOutput = ProtocolFrameOutput;

/// Room for a full frame of cell updates, so drawing costs a few writes
/// instead of one per queued command (stderr itself is unbuffered). Every
/// frame and every mode switch ends with an explicit flush.
const OUTPUT_BUFFER_BYTES: usize = 256 * 1024;

pub struct Tui {
  /// Never dropped: see `Drop for Tui`.
  terminal: ManuallyDrop<Terminal<CrosstermBackend<BufWriter<Stderr>>>>,
  protocol_renderer: ProtocolFrameRenderer,
  protocol_reset: Option<String>,
  suspended: bool,
  restored: bool,
}

impl Tui {
  pub fn new(protocol_reset: Option<String>) -> Result<Self> {
    enable_raw_mode()?;
    let mut stderr = BufWriter::with_capacity(OUTPUT_BUFFER_BYTES, io::stderr());
    execute!(
      stderr,
      EnterAlternateScreen,
      EnableMouseCapture,
      EnableBracketedPaste
    )?;
    let backend = CrosstermBackend::new(stderr);
    let mut terminal = Terminal::new(backend)?;
    reset_protocol_images(terminal.backend_mut(), protocol_reset.as_deref())?;
    Ok(Self {
      terminal: ManuallyDrop::new(terminal),
      protocol_renderer: ProtocolFrameRenderer::default(),
      protocol_reset,
      suspended: false,
      restored: false,
    })
  }

  pub fn draw<F>(&mut self, render: F) -> Result<()>
  where
    F: FnOnce(&mut Frame) -> FrameOutput,
  {
    self.protocol_renderer.draw(&mut self.terminal, render)
  }

  /// Returns the terminal to its normal state for good. Every step runs
  /// even when an earlier one fails, so a broken image cleanup cannot leave
  /// the shell in raw mode; the first error is reported.
  pub fn restore(&mut self) -> Result<()> {
    if self.restored {
      return Ok(());
    }
    self.restored = true;
    if self.suspended {
      return Ok(());
    }
    self.suspended = true;
    self.leave_terminal()
  }

  /// Temporarily hands the terminal to another program.
  pub fn suspend(&mut self) -> Result<()> {
    if self.suspended {
      return Ok(());
    }
    self.suspended = true;
    self.leave_terminal()
  }

  pub fn resume(&mut self) -> Result<()> {
    if !self.suspended {
      return Ok(());
    }
    enable_raw_mode()?;
    execute!(
      self.terminal.backend_mut(),
      EnterAlternateScreen,
      EnableMouseCapture,
      EnableBracketedPaste
    )?;
    self.terminal.clear()?;
    reset_protocol_images(self.terminal.backend_mut(), self.protocol_reset.as_deref())?;
    self.suspended = false;
    Ok(())
  }

  fn leave_terminal(&mut self) -> Result<()> {
    let images = self
      .protocol_renderer
      .clear_and_reset(self.terminal.backend_mut(), self.protocol_reset.as_deref());
    let raw_mode = disable_raw_mode().map_err(anyhow::Error::from);
    let cursor = self.terminal.show_cursor().map_err(anyhow::Error::from);
    let screen = execute!(
      self.terminal.backend_mut(),
      LeaveAlternateScreen,
      DisableMouseCapture,
      DisableBracketedPaste
    )
    .map_err(anyhow::Error::from);
    images.and(raw_mode).and(cursor).and(screen)
  }
}

impl Drop for Tui {
  /// Restores the terminal, except while unwinding from a panic: the panic
  /// hook has restored it already, and flushing whatever half-built frame
  /// is still buffered would print over the panic message. The terminal is
  /// deliberately leaked for the same reason; `restore` flushes all output.
  fn drop(&mut self) {
    if !thread::panicking() {
      let _ = self.restore();
    }
  }
}

/// A panic on the main thread restores the terminal before the panic
/// message is printed; otherwise the message lands on the alternate screen
/// and disappears. Panics on worker threads are logged instead of printed,
/// because stderr is the UI.
pub fn install_panic_hook() {
  let default_hook = std::panic::take_hook();
  std::panic::set_hook(Box::new(move |info| {
    if thread::current().name() == Some("main") {
      let _ = disable_raw_mode();
      let _ = execute!(
        io::stderr(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste,
        Show
      );
      default_hook(info);
    } else {
      tracing::error!(
        thread = thread::current().name().unwrap_or("unnamed"),
        panic = %info,
        "background thread panicked"
      );
    }
  }));
}
