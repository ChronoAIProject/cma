//! Bounded connection-pool latency summaries. Never retain or log commands, replies,
//! addresses, identifiers, credentials, database names, or driver errors.
use std::{
    collections::{hash_map::DefaultHasher, HashMap},
    hash::{Hash, Hasher},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use mongodb::{
    event::{cmap::CmapEvent, EventHandler},
    options::ClientOptions,
};

const WINDOW: Duration = Duration::from_secs(10);
const LABELS: [&str; 4] = [
    "checkout",
    "connection_ready",
    "pool_cleared",
    "connection_in_use",
];
const MAX_TRACKED_CONNECTIONS: usize = 4096;
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
    checked_out: HashMap<u64, Instant>,
    in_use_peak: usize,
    untracked: u64,
}

impl Window {
    fn new(started: Instant) -> Self {
        Self {
            started,
            samples: [Sample::default(); LABELS.len()],
            checked_out: HashMap::new(),
            in_use_peak: 0,
            untracked: 0,
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
struct Observer(Arc<Mutex<Window>>, &'static str);

impl Observer {
    fn checked_out(&self, key: u64, duration: Duration) {
        {
            let mut window = self.0.lock().unwrap_or_else(|error| error.into_inner());
            if window.checked_out.len() < MAX_TRACKED_CONNECTIONS {
                window.checked_out.insert(key, Instant::now());
                window.in_use_peak = window.in_use_peak.max(window.checked_out.len());
            } else {
                window.untracked = window.untracked.saturating_add(1);
                metrics::counter!("cma_mongo_connection_tracking_overflow_total", "pool" => self.1)
                    .increment(1);
            }
            metrics::gauge!("cma_mongo_connections_tracked", "pool" => self.1)
                .set(window.checked_out.len() as f64);
        }
        self.record(0, duration, false);
    }

    fn returned(&self, key: u64, closed: bool) {
        let started = {
            let mut window = self.0.lock().unwrap_or_else(|error| error.into_inner());
            let started = window.checked_out.remove(&key);
            metrics::gauge!("cma_mongo_connections_tracked", "pool" => self.1)
                .set(window.checked_out.len() as f64);
            started
        };
        if let Some(started) = started {
            self.record(3, started.elapsed(), closed);
        }
    }

    fn record(&self, index: usize, duration: Duration, failed: bool) {
        let operation = LABELS[index];
        let outcome = if failed { "failed" } else { "ok" };
        metrics::counter!("cma_mongo_pool_events_total", "pool" => self.1, "operation" => operation, "outcome" => outcome).increment(1);
        metrics::histogram!("cma_mongo_pool_operation_seconds", "pool" => self.1, "operation" => operation, "outcome" => outcome).record(duration.as_secs_f64());
        let now = Instant::now();
        let completed = {
            let mut window = self.0.lock().unwrap_or_else(|error| error.into_inner());
            window.record(index, duration, failed);
            (now.duration_since(window.started) >= WINDOW).then(|| {
                let mut next = Window::new(now);
                std::mem::swap(&mut next.checked_out, &mut window.checked_out);
                next.in_use_peak = next.checked_out.len();
                let in_use = next.checked_out.len();
                (std::mem::replace(&mut *window, next), in_use)
            })
        };
        if let Some((window, in_use)) = completed {
            // Release the lock before formatting or writing to the log sink.
            for (operation, sample) in LABELS.iter().zip(window.samples) {
                if sample.count == 0 {
                    continue;
                }
                tracing::info!(
                    parent: None,
                    pool = self.1,
                    operation,
                    window_ms = now.duration_since(window.started).as_millis(),
                    count = sample.count,
                    failures = sample.failures,
                    total_us = sample.total_us,
                    max_us = sample.max_us,
                    buckets = ?sample.buckets,
                    in_use,
                    in_use_peak = window.in_use_peak,
                    untracked = window.untracked,
                    "MongoDB client latency summary"
                );
            }
        }
    }
}

pub(super) fn observe(options: &mut ClientOptions) {
    options.cmap_event_handler = Some(EventHandler::callback(observer(options, "application")));
}

/// Each independently configured client keeps its own bounded occupancy state.
/// The fixed purpose label is never a URI, database, address or client identity.
pub(crate) fn observer(
    options: &ClientOptions,
    pool: &'static str,
) -> impl Fn(CmapEvent) + Send + Sync + 'static {
    metrics::gauge!("cma_mongo_connections_tracked", "pool" => pool).set(0);
    metrics::gauge!("cma_mongo_pool_max_connections", "pool" => pool)
        .set(options.max_pool_size.unwrap_or(10) as f64);
    let observer = Observer(Arc::new(Mutex::new(Window::new(Instant::now()))), pool);
    move |event| record_event(&observer, event)
}

#[cfg(test)]
fn attach(options: &mut ClientOptions, observer: Observer) {
    options.cmap_event_handler = Some(EventHandler::callback(move |event| {
        record_event(&observer, event)
    }));
}

fn record_event(observer: &Observer, event: CmapEvent) {
    // Command monitoring remains disabled: it decodes complete BSON commands/replies.
    match event {
        CmapEvent::ConnectionCheckoutStarted(_) => {
            // Cancellation can omit the end event; starts minus ends is not queue depth.
            metrics::counter!("cma_mongo_checkout_started_total", "pool" => observer.1).increment(1)
        }
        CmapEvent::ConnectionCheckedOut(event) => observer.checked_out(
            connection_key(&event.address, event.connection_id),
            event.duration,
        ),
        CmapEvent::ConnectionCheckedIn(event) => {
            observer.returned(connection_key(&event.address, event.connection_id), false)
        }
        CmapEvent::ConnectionClosed(event) => {
            observer.returned(connection_key(&event.address, event.connection_id), true)
        }
        CmapEvent::ConnectionCheckoutFailed(event) => observer.record(0, event.duration, true),
        CmapEvent::ConnectionReady(event) => observer.record(1, event.duration, false),
        CmapEvent::PoolCleared(_) => observer.record(2, Duration::ZERO, false),
        _ => {}
    }
}

// Driver IDs are only unique within a server's pool. Keep a bounded, ephemeral
// fingerprint for pairing; neither the address nor the fingerprint is logged.
pub(super) fn connection_key(address: &mongodb::options::ServerAddress, id: u32) -> u64 {
    let mut hash = DefaultHasher::new();
    (address, id).hash(&mut hash);
    hash.finish()
}

#[cfg(test)]
#[path = "mongo_latency_tests.rs"]
mod tests;
