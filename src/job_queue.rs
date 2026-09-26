//! Priority scheduling shared by the page rasterizer and the terminal
//! renderer: at most `max_concurrent` jobs run at once, one slot is always
//! kept free for visible work, and a queued preload can be promoted when it
//! becomes visible.

use std::{
  cmp::Ordering,
  collections::{BinaryHeap, HashMap},
  hash::Hash,
};

pub trait JobPriority: Ord + Copy {
  /// The priority of work the user is waiting for.
  const VISIBLE: Self;

  fn is_preload(self) -> bool {
    self < Self::VISIBLE
  }
}

pub struct JobQueue<K, P> {
  /// Current priority of every queued or running job.
  priorities: HashMap<K, P>,
  /// Running jobs and whether each was started as a preload.
  running: HashMap<K, bool>,
  running_preloads: usize,
  /// Queue entries; promotions push a second entry, so stale ones are
  /// skipped when popped.
  pending: BinaryHeap<Queued<K, P>>,
  sequence: u64,
  max_concurrent: usize,
}

struct Queued<K, P> {
  priority: P,
  sequence: u64,
  key: K,
}

impl<K, P: Ord> Ord for Queued<K, P> {
  /// Higher priority first, then first come, first served.
  fn cmp(&self, other: &Self) -> Ordering {
    self
      .priority
      .cmp(&other.priority)
      .then_with(|| other.sequence.cmp(&self.sequence))
  }
}

impl<K, P: Ord> PartialOrd for Queued<K, P> {
  fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
    Some(self.cmp(other))
  }
}

impl<K, P: Ord> PartialEq for Queued<K, P> {
  fn eq(&self, other: &Self) -> bool {
    self.cmp(other) == Ordering::Equal
  }
}

impl<K, P: Ord> Eq for Queued<K, P> {}

impl<K: Hash + Eq + Clone, P: JobPriority> JobQueue<K, P> {
  pub fn new(max_concurrent: usize) -> Self {
    Self {
      priorities: HashMap::new(),
      running: HashMap::new(),
      running_preloads: 0,
      pending: BinaryHeap::new(),
      sequence: 0,
      max_concurrent: max_concurrent.max(1),
    }
  }

  /// Preloads never take the last slot, so with one slot there are none.
  pub fn allows_preloads(&self) -> bool {
    self.max_preloads() > 0
  }

  /// Whether `key` is queued or running.
  pub fn contains(&self, key: &K) -> bool {
    self.priorities.contains_key(key)
  }

  pub fn running_count(&self) -> usize {
    self.running.len()
  }

  pub fn running_preloads(&self) -> usize {
    self.running_preloads
  }

  pub fn queued_count(&self) -> usize {
    self.pending.len()
  }

  /// Queues a job that is not queued or running yet.
  pub fn enqueue(&mut self, key: K, priority: P) {
    self.priorities.insert(key.clone(), priority);
    self.push(key, priority);
  }

  /// Raises a queued or running job to `priority`; returns whether it was
  /// lower. Raising a running job only affects its bookkeeping.
  pub fn promote(&mut self, key: &K, priority: P) -> bool {
    let Some(current) = self.priorities.get_mut(key) else {
      return false;
    };
    if *current >= priority {
      return false;
    }
    *current = priority;
    self.push(key.clone(), priority);
    true
  }

  /// Takes the next job that may start now and marks it running.
  pub fn start_next(&mut self) -> Option<(K, P)> {
    while self.running.len() < self.max_concurrent {
      let job = self.pending.pop()?;
      if self.running.contains_key(&job.key) {
        continue;
      }
      let Some(priority) = self.priorities.get(&job.key).copied() else {
        continue;
      };
      if job.priority < priority {
        continue;
      }
      let preload = priority.is_preload();
      if preload && self.running_preloads >= self.max_preloads() {
        // The heap is ordered by priority, so everything left is a preload.
        self.pending.push(job);
        return None;
      }
      if preload {
        self.running_preloads += 1;
      }
      self.running.insert(job.key.clone(), preload);
      return Some((job.key, priority));
    }
    None
  }

  /// Forgets a finished job. Unknown keys (e.g. jobs from before a
  /// `clear`) are ignored.
  pub fn finish(&mut self, key: &K) {
    self.priorities.remove(key);
    if self.running.remove(key) == Some(true) {
      self.running_preloads -= 1;
    }
  }

  /// Drops queued preloads that have not started; returns their keys.
  pub fn cancel_queued_preloads(&mut self) -> Vec<K> {
    self.pending.retain(|job| !job.priority.is_preload());
    let cancelled = self
      .priorities
      .iter()
      .filter(|(key, priority)| priority.is_preload() && !self.running.contains_key(*key))
      .map(|(key, _)| key.clone())
      .collect::<Vec<_>>();
    for key in &cancelled {
      self.priorities.remove(key);
    }
    cancelled
  }

  /// Forgets all jobs; results of jobs still running are ignored later.
  pub fn clear(&mut self) {
    self.priorities.clear();
    self.running.clear();
    self.running_preloads = 0;
    self.pending.clear();
    self.sequence = 0;
  }

  fn max_preloads(&self) -> usize {
    self.max_concurrent - 1
  }

  fn push(&mut self, key: K, priority: P) {
    let sequence = self.sequence;
    self.sequence = self.sequence.wrapping_add(1);
    self.pending.push(Queued {
      priority,
      sequence,
      key,
    });
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
  enum Priority {
    Preload,
    Visible,
  }

  impl JobPriority for Priority {
    const VISIBLE: Self = Self::Visible;
  }

  #[test]
  fn visible_jobs_go_first_and_keep_a_slot() {
    let mut queue = JobQueue::new(2);
    queue.enqueue("a", Priority::Preload);
    queue.enqueue("b", Priority::Preload);
    queue.enqueue("c", Priority::Visible);
    assert_eq!(queue.start_next(), Some(("c", Priority::Visible)));
    assert_eq!(queue.start_next(), Some(("a", Priority::Preload)));
    // Two running: nothing more starts.
    assert_eq!(queue.start_next(), None);
    queue.finish(&"c");
    // One preload runs and one slot is reserved for visible work.
    assert_eq!(queue.start_next(), None);
    queue.enqueue("d", Priority::Visible);
    assert_eq!(queue.start_next(), Some(("d", Priority::Visible)));
  }

  #[test]
  fn promotion_and_cancellation() {
    let mut queue = JobQueue::new(2);
    queue.enqueue("a", Priority::Preload);
    queue.enqueue("b", Priority::Preload);
    assert!(queue.promote(&"b", Priority::Visible));
    assert!(!queue.promote(&"b", Priority::Visible));
    assert_eq!(queue.start_next(), Some(("b", Priority::Visible)));
    // The stale preload entry of "b" is skipped; "a" is cancelled.
    assert_eq!(queue.cancel_queued_preloads(), vec!["a"]);
    assert!(!queue.contains(&"a"));
    assert!(queue.contains(&"b"));
    assert_eq!(queue.start_next(), None);
    queue.finish(&"b");
    assert_eq!(queue.running_count(), 0);
    // Finishing unknown jobs is harmless.
    queue.finish(&"zz");
    assert!(!JobQueue::<&str, Priority>::new(1).allows_preloads());
  }
}
