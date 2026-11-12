//! Opt-in bounded metadata evidence. Never perform I/O on a request/driver task.
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::time::Instant;

mod attempt;
mod output;
pub(crate) mod record;
mod ring;
pub(crate) use attempt::{Attempt, Progress, Timer};
use output::Output;
pub(crate) use record::{Event, Metadata, Stamp};
use ring::Ring;

const HISTORY: Duration = Duration::from_secs(30);
static RECORDER: OnceLock<Option<Arc<Recorder>>> = OnceLock::new();

pub(crate) fn configured() -> Option<Arc<Recorder>> {
    RECORDER.get_or_init(|| {
        let enabled = std::env::var(crate::config::MONGO_DIAGNOSTICS_ENABLED).ok();
        if !enabled_value(enabled.as_deref()) { return None; }
        match Output::start() {
            Ok(output) => Some(Recorder::new(output)),
            Err(_) => {
                metrics::counter!("cma_readiness_diagnostic_loss_total", "reason" => "writer_initialization").increment(1);
                None
            }
        }
    }).clone()
}

fn enabled_value(value: Option<&str>) -> bool {
    value == Some("true")
}

pub(crate) struct Recorder {
    origin: Instant,
    epoch_us: u64,
    process_hash: Arc<str>,
    sequence: AtomicU64,
    pub(super) loss: AtomicU64,
    follow_until: AtomicU64,
    ring: Mutex<Ring>,
    output: Output,
}

impl Recorder {
    fn new(output: Output) -> Arc<Self> {
        Arc::new(Self {
            origin: Instant::now(),
            epoch_us: micros(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default(),
            ),
            process_hash: uuid::Uuid::new_v4().simple().to_string().into(),
            sequence: AtomicU64::new(0),
            loss: AtomicU64::new(0),
            follow_until: AtomicU64::new(0),
            ring: Mutex::new(Ring::default()),
            output,
        })
    }

    pub(crate) fn next_id(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub(crate) fn stamp(&self) -> Stamp {
        let mono_us = micros(self.origin.elapsed());
        Stamp {
            mono_us,
            unix_us: self.epoch_us.saturating_add(mono_us),
            sequence: self.next_id(),
        }
    }

    pub(crate) fn observe(&self, metadata: Metadata, event: &'static str, duration: Duration) {
        let row = Arc::new(Event {
            stamp: self.stamp(),
            metadata,
            event,
            duration_us: micros(duration),
        });
        match self.ring.try_lock() {
            Ok(mut ring) => ring.push(row.clone()),
            Err(_) => self.lost("ring_busy"),
        }
        // An event in overlapping post-deadline windows is emitted once. Snapshots
        // retain source sequence numbers so consumers can deduplicate overlap.
        let follow_until = self.follow_until.load(Ordering::Relaxed);
        if follow_until != 0 && row.stamp.mono_us <= follow_until {
            self.output.event(self.process_hash.clone(), row);
        }
    }

    pub(super) fn lost(&self, reason: &'static str) {
        self.loss.fetch_add(1, Ordering::Relaxed);
        metrics::counter!("cma_readiness_diagnostic_loss_total", "reason" => reason).increment(1);
    }

    fn freeze(&self, check: u64) {
        let at = self.stamp();
        self.follow_until.fetch_max(
            at.mono_us.saturating_add(micros(HISTORY)),
            Ordering::Relaxed,
        );
        match self.ring.try_lock() {
            Ok(mut ring) => {
                let mut snapshot = ring.freeze(at, check, self.loss.load(Ordering::Relaxed));
                drop(ring);
                snapshot.lost = self.loss.load(Ordering::Relaxed);
                self.output.snapshot(self.process_hash.clone(), snapshot);
            }
            Err(_) => self.lost("snapshot_busy"),
        }
    }

    pub(super) fn record(&self, record: record::Record) {
        self.output.record(self.process_hash.clone(), record);
    }
}

pub(super) fn micros(value: Duration) -> u64 {
    value.as_micros().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
pub(crate) mod fixture;
#[cfg(test)]
mod tests;
