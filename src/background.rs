//! Background threads feeding the event loop: terminal input and the
//! optional file watcher behind automatic refresh.

use std::{
  fs,
  path::PathBuf,
  sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
  },
  thread,
  time::{Duration, Instant, SystemTime},
};

use crossterm::event::{self as crossterm_event, Event, MouseEventKind};
use tokio::sync::mpsc;

use crate::{config::BehaviorConfig, event::AsyncEvent};

const INPUT_POLL: Duration = Duration::from_millis(50);
const PAUSE_WAIT_LIMIT: Duration = Duration::from_millis(300);

/// Controls the terminal input thread. While paused (an external editor
/// owns the terminal) the thread stops reading, so it cannot steal the
/// editor's keystrokes; events read before a pause carry an older
/// generation and are dropped by the event loop.
pub struct InputGate {
  shared: Arc<InputShared>,
}

struct InputShared {
  enabled: AtomicBool,
  /// Set by the input thread once it has noticed a pause and stopped
  /// polling the terminal.
  parked: AtomicBool,
  generation: AtomicU64,
}

impl InputGate {
  pub fn spawn(tx: mpsc::UnboundedSender<AsyncEvent>) -> Self {
    let shared = Arc::new(InputShared {
      enabled: AtomicBool::new(true),
      parked: AtomicBool::new(false),
      generation: AtomicU64::new(0),
    });
    let thread_shared = shared.clone();
    thread::spawn(move || read_input(&thread_shared, &tx));
    Self { shared }
  }

  pub fn generation(&self) -> u64 {
    self.shared.generation.load(Ordering::SeqCst)
  }

  /// Stops input reading and waits (bounded) until the thread is idle.
  pub fn pause(&self) {
    self.shared.enabled.store(false, Ordering::SeqCst);
    self.shared.generation.fetch_add(1, Ordering::SeqCst);
    let started = Instant::now();
    while !self.shared.parked.load(Ordering::SeqCst) && started.elapsed() < PAUSE_WAIT_LIMIT {
      thread::sleep(Duration::from_millis(2));
    }
  }

  pub fn resume(&self) {
    self.shared.generation.fetch_add(1, Ordering::SeqCst);
    self.shared.enabled.store(true, Ordering::SeqCst);
  }
}

fn read_input(shared: &InputShared, tx: &mpsc::UnboundedSender<AsyncEvent>) {
  loop {
    if !shared.enabled.load(Ordering::SeqCst) {
      shared.parked.store(true, Ordering::SeqCst);
      thread::sleep(Duration::from_millis(10));
      continue;
    }
    shared.parked.store(false, Ordering::SeqCst);
    // Re-check after announcing activity: a pause that saw `parked` still
    // set must not race with a poll starting here.
    if !shared.enabled.load(Ordering::SeqCst) {
      continue;
    }
    // Poll with a timeout instead of blocking in `read`, so a pause takes
    // effect without waiting for the next keystroke.
    match crossterm_event::poll(INPUT_POLL) {
      Ok(true) => {}
      Ok(false) => continue,
      Err(_) => {
        thread::sleep(Duration::from_millis(10));
        continue;
      }
    }
    if !shared.enabled.load(Ordering::SeqCst) {
      continue;
    }
    match crossterm_event::read() {
      // Pointer motion without a button changes nothing; skip it early
      // instead of waking the event loop for every mouse movement.
      Ok(Event::Mouse(mouse)) if mouse.kind == MouseEventKind::Moved => {}
      Ok(event) => {
        let generation = shared.generation.load(Ordering::SeqCst);
        if tx.send(AsyncEvent::Input { event, generation }).is_err() {
          break;
        }
      }
      Err(_) => thread::sleep(Duration::from_millis(10)),
    }
  }
}

/// Drops terminal events queued while an external program ran.
pub fn discard_pending_terminal_events() {
  while crossterm_event::poll(Duration::from_millis(0)).unwrap_or(false) {
    if crossterm_event::read().is_err() {
      break;
    }
  }
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
