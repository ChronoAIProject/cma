//! Bounded driver latency summaries. Never retain or log commands, replies,
//! addresses, identifiers, credentials, database names, or driver errors.
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use mongodb::{
    event::{cmap::CmapEvent, command::CommandEvent, EventHandler},
    options::ClientOptions,
};

const WINDOW: Duration = Duration::from_secs(10);
const LABELS: [&str; 12] = [
    "find",
    "aggregate",
    "getMore",
    "insert",
    "update",
    "delete",
    "findAndModify",
    "commitTransaction",
    "other_command",
    "checkout",
    "connection_ready",
    "pool_cleared",
];
const LIMITS_US: [u64; 6] = [1_000, 5_000, 25_000, 100_000, 250_000, 1_000_000];

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
struct Sample {
    count: u64,
    failures: u64,
    total_us: u64,
    max_us: u64,
    buckets: [u64; 7],
}

struct Window {
    started: Instant,
    samples: [Sample; LABELS.len()],
}

impl Window {
    fn new(started: Instant) -> Self {
        Self {
            started,
            samples: [Sample::default(); LABELS.len()],
        }
    }

    fn record(&mut self, index: usize, duration: Duration, failed: bool) {
        let elapsed = u64::try_from(duration.as_micros()).unwrap_or(u64::MAX);
        let sample = &mut self.samples[index];
        sample.count = sample.count.saturating_add(1);
        sample.failures = sample.failures.saturating_add(u64::from(failed));
        sample.total_us = sample.total_us.saturating_add(elapsed);
        sample.max_us = sample.max_us.max(elapsed);
        let bucket = LIMITS_US.partition_point(|limit| elapsed > *limit);
        sample.buckets[bucket] = sample.buckets[bucket].saturating_add(1);
    }
}

#[derive(Clone)]
struct Observer(Arc<Mutex<Window>>);

impl Observer {
    fn record(&self, index: usize, duration: Duration, failed: bool) {
        let now = Instant::now();
        let completed = {
            let mut window = self.0.lock().unwrap_or_else(|error| error.into_inner());
            window.record(index, duration, failed);
            (now.duration_since(window.started) >= WINDOW)
                .then(|| std::mem::replace(&mut *window, Window::new(now)))
        };
        if let Some(window) = completed {
            // Release the lock before formatting or writing to the log sink.
            for (operation, sample) in LABELS.iter().zip(window.samples) {
                if sample.count == 0 {
                    continue;
                }
                tracing::info!(
                    parent: None,
                    operation,
                    window_ms = now.duration_since(window.started).as_millis(),
                    count = sample.count,
                    failures = sample.failures,
                    total_us = sample.total_us,
                    max_us = sample.max_us,
                    buckets = ?sample.buckets,
                    "MongoDB client latency summary"
                );
            }
        }
    }
}

fn command_index(name: &str) -> usize {
    LABELS[..8]
        .iter()
        .position(|label| *label == name)
        .unwrap_or(8)
}

pub(super) fn observe(options: &mut ClientOptions) {
    let observer = Observer(Arc::new(Mutex::new(Window::new(Instant::now()))));
    attach(options, observer);
}

fn attach(options: &mut ClientOptions, observer: Observer) {
    let commands = observer.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| match event {
        CommandEvent::Succeeded(event) => {
            commands.record(command_index(&event.command_name), event.duration, false);
        }
        CommandEvent::Failed(event) => {
            commands.record(command_index(&event.command_name), event.duration, true);
        }
        _ => {}
    }));
    options.cmap_event_handler = Some(EventHandler::callback(move |event| match event {
        CmapEvent::ConnectionCheckedOut(event) => observer.record(9, event.duration, false),
        CmapEvent::ConnectionCheckoutFailed(event) => observer.record(9, event.duration, true),
        CmapEvent::ConnectionReady(event) => observer.record(10, event.duration, false),
        CmapEvent::PoolCleared(_) => observer.record(11, Duration::ZERO, false),
        _ => {}
    }));
}

#[cfg(test)]
#[path = "mongo_latency_tests.rs"]
mod tests;
