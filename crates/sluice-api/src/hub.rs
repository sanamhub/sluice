//! The events of one job, handed to the watchers of that job through buffers that never grow past a
//! bound. A watcher that falls behind loses the newest events, and the gap is one `dropped` event
//! that says how many, in the place where they would have been.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::Notify;

use crate::proto::{self, event::Event as Kind};

/// How many events one watcher may have waiting. The one terminal event and one gap marker come on
/// top of this, so a buffer never holds more than `WATCH_BUFFER + 2` items.
pub const WATCH_BUFFER: usize = 256;

/// How many watchers one job has at once.
pub const MAX_WATCHERS: usize = 8;

enum Item {
    Event(proto::Event),
    Gap(u64),
}

#[derive(Default)]
struct Queue {
    items: VecDeque<Item>,
    events: usize,
    closed: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    wake: Notify,
}

fn lock(queue: &Mutex<Queue>) -> MutexGuard<'_, Queue> {
    // A panic while holding the lock leaves a queue that is still well formed.
    queue
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Shared {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            queue: Mutex::new(Queue::default()),
            wake: Notify::new(),
        })
    }

    /// Adds an event, or counts it as dropped when the buffer is full.
    fn push(&self, event: &proto::Event) {
        let mut queue = lock(&self.queue);
        if queue.events >= WATCH_BUFFER {
            match queue.items.back_mut() {
                Some(Item::Gap(count)) => *count += 1,
                _ => queue.items.push_back(Item::Gap(1)),
            }
        } else {
            queue.events += 1;
            queue.items.push_back(Item::Event(event.clone()));
        }
        drop(queue);
        self.wake.notify_one();
    }

    /// Adds the last event, which is kept even when the buffer is full, and ends the stream.
    fn finish(&self, event: &proto::Event) {
        let mut queue = lock(&self.queue);
        queue.events += 1;
        queue.items.push_back(Item::Event(event.clone()));
        queue.closed = true;
        drop(queue);
        self.wake.notify_one();
    }
}

/// The reading end of one watcher's buffer.
pub struct Watcher {
    job_id: String,
    shared: Arc<Shared>,
}

impl Watcher {
    /// The next event, or `None` once the job has ended and everything was read. A gap comes as a
    /// `dropped` event carrying the count.
    pub async fn next(&mut self) -> Option<proto::Event> {
        loop {
            {
                let mut queue = lock(&self.shared.queue);
                match queue.items.pop_front() {
                    Some(Item::Event(event)) => {
                        queue.events -= 1;
                        return Some(event);
                    }
                    Some(Item::Gap(count)) => {
                        return Some(event(&self.job_id, Kind::Dropped(proto::Dropped { count })));
                    }
                    None if queue.closed => return None,
                    None => {}
                }
            }
            self.shared.wake.notified().await;
        }
    }

    /// How many items wait in this buffer, for a test that checks the bound.
    #[cfg(test)]
    pub(crate) fn waiting(&self) -> usize {
        lock(&self.shared.queue).items.len()
    }
}

/// An event of `job_id` stamped with the daemon's clock.
pub(crate) fn event(job_id: &str, kind: Kind) -> proto::Event {
    proto::Event {
        job_id: job_id.to_string(),
        utc_ms: sluice_core::engine::now_ms(),
        event: Some(kind),
    }
}

/// The watchers of one job.
#[derive(Default)]
pub(crate) struct Hub {
    inner: Mutex<HubState>,
}

#[derive(Default)]
struct HubState {
    watchers: Vec<Arc<Shared>>,
    last: Option<proto::Event>,
}

impl Hub {
    /// A new watcher, from now on. A job that has ended gives one that holds its last event.
    pub(crate) fn watch(&self, job_id: &str) -> Result<Watcher, crate::ApiError> {
        let mut state = self.state();
        let shared = Shared::new();
        if let Some(last) = &state.last {
            shared.finish(last);
        } else {
            // A watcher whose reader has gone holds the only other count of its buffer.
            state.watchers.retain(|w| Arc::strong_count(w) > 1);
            if state.watchers.len() >= MAX_WATCHERS {
                return Err(crate::ApiError::Busy(format!(
                    "a job has at most {MAX_WATCHERS} watchers at once"
                )));
            }
            state.watchers.push(shared.clone());
        }
        Ok(Watcher {
            job_id: job_id.to_string(),
            shared,
        })
    }

    pub(crate) fn publish(&self, event: &proto::Event) {
        let state = self.state();
        for watcher in &state.watchers {
            watcher.push(event);
        }
    }

    /// Ends every watcher after this last event, and keeps it for a watcher that comes later.
    pub(crate) fn finish(&self, event: &proto::Event) {
        let mut state = self.state();
        state.last = Some(event.clone());
        for watcher in state.watchers.drain(..) {
            watcher.finish(event);
        }
    }

    fn state(&self) -> MutexGuard<'_, HubState> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests;
