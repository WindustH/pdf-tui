//! Background threads feeding the event loop: terminal input and the
//! optional file watcher behind automatic refresh.

use std::{
  fs, io,
  path::PathBuf,
  thread,
  time::{Duration, Instant, SystemTime},
};

use crossterm::event::{Event, MouseEventKind};
use framework_tui::InputReader;
use tokio::sync::mpsc;

use crate::{config::BehaviorConfig, event::AsyncEvent};

/// Forwards terminal input to the event loop. Pointer motion without a
/// button changes nothing, so it is dropped here instead of waking the
/// event loop for every mouse movement.
pub fn spawn_input_reader(tx: mpsc::UnboundedSender<AsyncEvent>) -> io::Result<InputReader> {
  InputReader::spawn(move |input| {
    if matches!(&input.event, Event::Mouse(mouse) if mouse.kind == MouseEventKind::Moved) {
      return true;
    }
    tx.send(AsyncEvent::Input {
      event: input.event,
      generation: input.generation,
    })
    .is_ok()
  })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileSignature {
  len: u64,
  modified: Option<SystemTime>,
}

fn file_signature(path: &PathBuf) -> Option<FileSignature> {
  let metadata = fs::metadata(path).ok()?;
  Some(FileSignature {
    len: metadata.len(),
    modified: metadata.modified().ok(),
  })
}

/// Polls `path` and requests a refresh after it changes, at most once per
/// `auto_refresh_min_interval_ms` so bursts of writes collapse into one.
pub fn spawn_file_watcher(
  tx: mpsc::UnboundedSender<AsyncEvent>,
  path: PathBuf,
  behavior: &BehaviorConfig,
) {
  if !behavior.auto_refresh {
    return;
  }
  let poll = Duration::from_millis(behavior.auto_refresh_poll_ms.max(200));
  let min_interval = Duration::from_millis(behavior.auto_refresh_min_interval_ms.max(500));
  thread::spawn(move || {
    let mut last_signature = file_signature(&path);
    let mut last_sent = Instant::now()
      .checked_sub(min_interval)
      .unwrap_or_else(Instant::now);
    let mut pending = false;
    loop {
      thread::sleep(poll);
      let signature = file_signature(&path);
      if signature != last_signature {
        last_signature = signature;
        pending = true;
      }
      if pending && last_sent.elapsed() >= min_interval {
        if tx.send(AsyncEvent::AutoRefreshRequested).is_err() {
          break;
        }
        last_sent = Instant::now();
        pending = false;
      }
    }
  });
}
