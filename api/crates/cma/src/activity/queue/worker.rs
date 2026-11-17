//! One batch at a time, retaining capacity until majority persistence succeeds.
use super::{Entry, Reservation, Shared};
use layer2_core::{ActivityRecord, ActivityStore};
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
    time::{Instant, MissedTickBehavior},
};

const MAX_BATCH: usize = 256;
const BATCH_WAIT: Duration = Duration::from_millis(250);
const INITIAL_RETRY: Duration = Duration::from_millis(100);
const MAX_RETRY: Duration = Duration::from_secs(5);

pub(crate) struct ActivityWorker {
    store: Arc<dyn ActivityStore>,
    receiver: mpsc::Receiver<Entry>,
    shared: Arc<Shared>,
    draining: bool,
}

#[derive(Default)]
struct Batch {
    records: Vec<ActivityRecord>,
    reservations: Vec<Reservation>,
    oldest: Option<Instant>,
}

impl Batch {
    fn push(&mut self, entry: Entry) {
        let enqueued = entry.reservation.enqueued;
        self.oldest = Some(self.oldest.map_or(enqueued, |oldest| oldest.min(enqueued)));
        self.records.push(entry.record);
        self.reservations.push(entry.reservation);
    }
    fn oldest(&self) -> Instant {
        self.oldest.expect("nonempty batch")
    }
    fn persisted(&mut self) {
        for entry in &mut self.reservations {
            entry.persisted = true;
        }
    }
}

impl ActivityWorker {
    pub(super) fn new(
        store: Arc<dyn ActivityStore>,
        receiver: mpsc::Receiver<Entry>,
        shared: Arc<Shared>,
    ) -> Self {
        Self {
            store,
            receiver,
            shared,
            draining: false,
        }
    }

    pub(crate) fn spawn(self, shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(self.run(shutdown))
    }

    fn close(&mut self) {
        self.draining = true;
        self.shared.accepting.store(false, Ordering::Release);
        self.receiver.close();
        tracing::info!(
            outstanding = self.shared.records.load(Ordering::Acquire),
            "draining request activity queue"
        );
    }

    async fn run(mut self, shutdown: watch::Receiver<bool>) {
        let mut ticks = tokio::time::interval(BATCH_WAIT);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            let mut batch = Batch::default();
            // Consume an already-ready backlog before the timer: old queued rows
            // must still form full batches after recovery from a storage outage.
            while batch.records.len() < MAX_BATCH {
                let deadline = batch.oldest.map(|enqueued| enqueued + BATCH_WAIT);
                tokio::select! {
                    biased;
                    _ = crate::shutdown::signalled(shutdown.clone()), if !self.draining => self.close(),
                    entry = self.receiver.recv() => match entry {
                        Some(entry) => {
                            batch.push(entry);
                            self.shared.metrics.oldest.set(batch.oldest().elapsed().as_secs_f64());
                        }
                        None => break,
                    },
                    _ = async { tokio::time::sleep_until(deadline.expect("guarded deadline")).await }, if deadline.is_some() && !self.draining => break,
                }
            }
            if batch.records.is_empty() {
                break;
            }
            let mut retry = INITIAL_RETRY;
            let mut failures = 0_u64;
            loop {
                let store = self.store.clone();
                let started = Instant::now();
                self.shared
                    .metrics
                    .batch_size
                    .record(batch.records.len() as f64);
                let write = store.append_many(&batch.records);
                tokio::pin!(write);
                let result = loop {
                    tokio::select! {
                        biased;
                        _ = crate::shutdown::signalled(shutdown.clone()), if !self.draining => self.close(),
                        _ = ticks.tick() => self.shared.metrics.oldest.set(batch.oldest().elapsed().as_secs_f64()),
                        result = &mut write => break result,
                    }
                };
                self.shared
                    .metrics
                    .write_seconds
                    .record(started.elapsed().as_secs_f64());
                if result.is_ok() {
                    break;
                }
                failures = failures.saturating_add(1);
                if failures.is_power_of_two() {
                    // Mongo error strings may contain record IDs or documents.
                    tracing::warn!(
                        failures,
                        retry_ms = retry.as_millis() as u64,
                        "request activity batch failed; retaining batch for retry"
                    );
                }
                let pause = tokio::time::sleep(retry);
                tokio::pin!(pause);
                loop {
                    tokio::select! {
                        biased;
                        _ = crate::shutdown::signalled(shutdown.clone()), if !self.draining => self.close(),
                        _ = ticks.tick() => self.shared.metrics.oldest.set(batch.oldest().elapsed().as_secs_f64()),
                        _ = &mut pause => break,
                    }
                }
                retry = (retry * 2).min(MAX_RETRY);
            }
            batch.persisted();
            drop(batch);
            self.shared.metrics.oldest.set(0);
        }
        tracing::info!("request activity queue drained");
    }
}

impl Drop for ActivityWorker {
    fn drop(&mut self) {
        self.shared.accepting.store(false, Ordering::Release);
        self.receiver.close();
        // Release queued reservations before exporting final gauges on abort.
        while self.receiver.try_recv().is_ok() {}
        self.shared.metrics.oldest.set(0);
    }
}
