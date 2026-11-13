//! Nonblocking request telemetry admission. Budgets include batches in flight.

use std::{
    io::{self, Write},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

use layer2_core::{ActivityRecord, ActivityStore};
use tokio::{sync::mpsc, time::Instant};

mod metrics;
mod worker;
pub(crate) use worker::ActivityWorker;

pub(crate) const MAX_RECORDS: usize = 10_000;
pub(crate) const MAX_BYTES: usize = 16 * 1024 * 1024;

pub(crate) trait ActivitySink: Send + Sync {
    /// Drops the new record on exhaustion or shutdown; never waits for storage.
    fn try_enqueue(&self, record: ActivityRecord) -> bool;
}

pub(crate) struct ActivityQueue {
    sender: mpsc::Sender<Entry>,
    shared: Arc<Shared>,
}

struct Shared {
    accepting: AtomicBool,
    records: AtomicUsize,
    bytes: AtomicUsize,
    max_records: usize,
    max_bytes: usize,
    metrics: metrics::QueueMetrics,
}

struct Entry {
    record: ActivityRecord,
    reservation: Reservation,
}

struct Reservation {
    shared: Arc<Shared>,
    bytes: usize,
    enqueued: Instant,
    persisted: bool,
}

impl ActivityQueue {
    pub(crate) fn new(store: Arc<dyn ActivityStore>) -> (Arc<Self>, ActivityWorker) {
        Self::bounded(store, MAX_RECORDS, MAX_BYTES)
    }

    fn bounded(
        store: Arc<dyn ActivityStore>,
        max_records: usize,
        max_bytes: usize,
    ) -> (Arc<Self>, ActivityWorker) {
        let (sender, receiver) = mpsc::channel(max_records);
        let shared = Arc::new(Shared {
            accepting: AtomicBool::new(true),
            records: AtomicUsize::new(0),
            bytes: AtomicUsize::new(0),
            max_records,
            max_bytes,
            metrics: metrics::QueueMetrics::new(),
        });
        let queue = Arc::new(Self {
            sender,
            shared: shared.clone(),
        });
        (queue, ActivityWorker::new(store, receiver, shared))
    }
}

impl ActivitySink for ActivityQueue {
    fn try_enqueue(&self, record: ActivityRecord) -> bool {
        let shared = &self.shared;
        if !shared.accepting.load(Ordering::Acquire) {
            shared.metrics.drop_records(metrics::Reason::Shutdown, 1);
            return false;
        }
        // Count serialized bytes without allocating a second copy of the payload.
        let mut size = SerializedSize(0);
        if serde_json::to_writer(&mut size, &record).is_err() {
            tracing::error!("request activity could not be sized for queue admission");
            shared.metrics.drop_records(metrics::Reason::ByteBudget, 1);
            return false;
        }
        if !reserve(&shared.records, 1, shared.max_records) {
            shared.metrics.drop_records(metrics::Reason::QueueFull, 1);
            return false;
        }
        if !reserve(&shared.bytes, size.0, shared.max_bytes) {
            shared.records.fetch_sub(1, Ordering::AcqRel);
            shared.metrics.drop_records(metrics::Reason::ByteBudget, 1);
            return false;
        }
        shared.metrics.records.increment(1.0);
        shared.metrics.bytes.increment(size.0 as f64);
        let entry = Entry {
            record,
            reservation: Reservation {
                shared: shared.clone(),
                bytes: size.0,
                enqueued: Instant::now(),
                persisted: false,
            },
        };
        match self.sender.try_send(entry) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Closed(_)) => false,
            Err(mpsc::error::TrySendError::Full(mut entry)) => {
                // The reservation bounds normally make channel fullness impossible.
                entry.reservation.persisted = true;
                shared.metrics.drop_records(metrics::Reason::QueueFull, 1);
                false
            }
        }
    }
}

fn reserve(counter: &AtomicUsize, amount: usize, limit: usize) -> bool {
    counter
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            current.checked_add(amount).filter(|next| *next <= limit)
        })
        .is_ok()
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.shared.records.fetch_sub(1, Ordering::AcqRel);
        self.shared.bytes.fetch_sub(self.bytes, Ordering::AcqRel);
        self.shared.metrics.records.decrement(1.0);
        self.shared.metrics.bytes.decrement(self.bytes as f64);
        // Also runs when the coordinator aborts a stalled write at its deadline.
        if !self.persisted {
            self.shared
                .metrics
                .drop_records(metrics::Reason::Shutdown, 1);
        }
    }
}

struct SerializedSize(usize);
impl Write for SerializedSize {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
