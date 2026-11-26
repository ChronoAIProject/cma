//! Bounded connection-pool latency summaries. Never retain or log commands, replies,
//! addresses, identifiers, credentials, database names, or driver errors.
use std::{
    collections::{
        hash_map::{DefaultHasher, Entry},
        HashMap,
    },
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

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct PoolConnection {
    server: u64,
    id: u32,
}

impl PoolConnection {
    fn new(address: &mongodb::options::ServerAddress, id: u32) -> Self {
        let mut hash = DefaultHasher::new();
        address.hash(&mut hash);
        Self {
            server: hash.finish(),
            id,
        }
    }
}

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
    checked_out: HashMap<PoolConnection, Instant>,
    server_in_use: HashMap<u64, usize>,
    in_use_peak: usize,
    untracked: u64,
    tracking_incomplete: bool,
}

impl Window {
    fn new(started: Instant) -> Self {
        super::mongo_diagnostics::output::initialize();
        Self {
            started,
            samples: [Sample::default(); LABELS.len()],
            checked_out: HashMap::new(),
            server_in_use: HashMap::new(),
            in_use_peak: 0,
            untracked: 0,
            tracking_incomplete: false,
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
struct Observer(Arc<Mutex<Window>>, &'static str, u32);

impl Observer {
    fn checked_out(&self, connection: PoolConnection, duration: Duration) {
        let pool_state = {
            let mut window = self.0.lock().unwrap_or_else(|error| error.into_inner());
            let pool_state = if window.tracking_incomplete
                || window.checked_out.len() >= MAX_TRACKED_CONNECTIONS
            {
                "unknown"
            } else if window
                .server_in_use
                .get(&connection.server)
                .copied()
                .unwrap_or(0)
                + 1
                >= self.2 as usize
            {
                "at_limit"
            } else {
                "below_limit"
            };
            if window.checked_out.len() < MAX_TRACKED_CONNECTIONS {
                if window
                    .checked_out
                    .insert(connection, Instant::now())
                    .is_none()
                {
                    *window.server_in_use.entry(connection.server).or_default() += 1;
                }
                window.in_use_peak = window.in_use_peak.max(window.checked_out.len());
            } else {
                window.untracked = window.untracked.saturating_add(1);
                window.tracking_incomplete = true;
                metrics::counter!("cma_mongo_connection_tracking_overflow_total", "pool" => self.1)
                    .increment(1);
            }
            metrics::gauge!("cma_mongo_connections_tracked", "pool" => self.1)
                .set(window.checked_out.len() as f64);
            pool_state
        };
        metrics::histogram!("cma_mongo_checkout_wait_seconds", "pool" => self.1, "pool_state" => pool_state)
            .record(duration.as_secs_f64());
        self.record(0, duration, false);
    }

    fn returned(&self, connection: PoolConnection, closed: bool) {
        let started = {
            let mut window = self.0.lock().unwrap_or_else(|error| error.into_inner());
            let started = window.checked_out.remove(&connection);
            if let (Some(_), Entry::Occupied(mut count)) =
                (started, window.server_in_use.entry(connection.server))
            {
                if *count.get() == 1 {
                    count.remove();
                } else {
                    *count.get_mut() -= 1;
                }
            }
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
                // An untracked connection can outlive a summary window. With
                // no pairing key we cannot prove occupancy complete again.
                next.tracking_incomplete = window.tracking_incomplete;
                std::mem::swap(&mut next.checked_out, &mut window.checked_out);
                std::mem::swap(&mut next.server_in_use, &mut window.server_in_use);
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
                super::mongo_diagnostics::output::send(
                    super::mongo_diagnostics::output::Record::Latency {
                        pool: self.1,
                        operation,
                        window_ms: now.duration_since(window.started).as_millis() as u64,
                        count: sample.count,
                        failures: sample.failures,
                        total_us: sample.total_us,
                        max_us: sample.max_us,
                        buckets: sample.buckets,
                        in_use,
                        in_use_peak: window.in_use_peak,
                        untracked: window.untracked,
                    },
                );
            }
        }
    }
}

pub(super) fn observe(options: &mut ClientOptions) {
    let pool_observer = observer(options, "application");
    let diagnostics = super::mongo_diagnostics::install(options, "general");
    options.cmap_event_handler = Some(EventHandler::callback(move |event| {
        if let Some(diagnostics) = &diagnostics {
            diagnostics.cmap(&event);
        }
        pool_observer(event);
    }));
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
    let observer = Observer(
        Arc::new(Mutex::new(Window::new(Instant::now()))),
        pool,
        options.max_pool_size.unwrap_or(10),
    );
    move |event| record_event(&observer, event)
}

#[cfg(test)]
fn attach(options: &mut ClientOptions, observer: Observer) {
    // This fixture exercises monitoring explicitly, independent of process flags.
    let diagnostics =
        super::mongo_diagnostics::install_configured(options, "general", Some("true"));
    options.cmap_event_handler = Some(EventHandler::callback(move |event| {
        if let Some(diagnostics) = &diagnostics {
            diagnostics.cmap(&event);
        }
        record_event(&observer, event)
    }));
}

fn record_event(observer: &Observer, event: CmapEvent) {
    // Diagnostic command monitoring is composed separately from pool metrics.
    match event {
        CmapEvent::ConnectionCheckoutStarted(_) => {
            // Cancellation can omit the end event; starts minus ends is not queue depth.
            metrics::counter!("cma_mongo_checkout_started_total", "pool" => observer.1).increment(1)
        }
        CmapEvent::ConnectionCheckedOut(event) => observer.checked_out(
            PoolConnection::new(&event.address, event.connection_id),
            event.duration,
        ),
        CmapEvent::ConnectionCheckedIn(event) => observer.returned(
            PoolConnection::new(&event.address, event.connection_id),
            false,
        ),
        CmapEvent::ConnectionClosed(event) => observer.returned(
            PoolConnection::new(&event.address, event.connection_id),
            true,
        ),
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

#[cfg(test)]
#[path = "mongo_checkout_tests.rs"]
mod checkout_tests;
