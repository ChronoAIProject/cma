//! Fixed process-local desktop relay observations for the private diagnostics.
use std::{
    fmt::Write as _,
    sync::{
        atomic::{AtomicU64, Ordering::Relaxed},
        Mutex, PoisonError,
    },
    time::Duration,
};
use tokio::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CloseReason {
    ClientClosed,
    ClientError,
    RuntimeClosed,
    RuntimeError,
    ClientSendTimeout,
    RuntimeSendTimeout,
    AuthorizationRefused,
    Lifetime,
}

impl CloseReason {
    const ALL: [Self; 8] = [
        Self::ClientClosed,
        Self::ClientError,
        Self::RuntimeClosed,
        Self::RuntimeError,
        Self::ClientSendTimeout,
        Self::RuntimeSendTimeout,
        Self::AuthorizationRefused,
        Self::Lifetime,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::ClientClosed => "client_closed",
            Self::ClientError => "client_error",
            Self::RuntimeClosed => "runtime_closed",
            Self::RuntimeError => "runtime_error",
            Self::ClientSendTimeout => "client_send_timeout",
            Self::RuntimeSendTimeout => "runtime_send_timeout",
            Self::AuthorizationRefused => "authorization_refused",
            Self::Lifetime => "lifetime",
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum Direction {
    Uplink,
    Downlink,
}

impl Direction {
    const ALL: [Self; 2] = [Self::Uplink, Self::Downlink];

    fn label(self) -> &'static str {
        match self {
            Self::Uplink => "uplink",
            Self::Downlink => "downlink",
        }
    }
}

// Include the issue's attribution boundaries (20 ms and 100 ms). The final
// counter retains samples beyond the explicit finite buckets.
const SEND_WAIT_BUCKETS_MS: [u64; 13] = [
    1, 2, 5, 10, 20, 50, 100, 250, 500, 1_000, 2_000, 5_000, 10_000,
];
const BUCKET_COUNT: usize = SEND_WAIT_BUCKETS_MS.len() + 1;
const SEND_WAIT_SECONDS: &str = "cma_desktop_relay_send_wait_seconds";

#[derive(Default)]
struct SendWait {
    buckets: [AtomicU64; BUCKET_COUNT],
    sum_nanos: AtomicU64,
}

impl SendWait {
    fn record(&self, elapsed: Duration) {
        let nanos = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
        let index = SEND_WAIT_BUCKETS_MS.partition_point(|bound| bound * 1_000_000 < nanos);
        self.buckets[index].fetch_add(1, Relaxed);
        let _ = self
            .sum_nanos
            .fetch_update(Relaxed, Relaxed, |sum| Some(sum.saturating_add(nanos)));
    }

    fn snapshot(&self) -> SendWaitSnapshot {
        SendWaitSnapshot {
            buckets: std::array::from_fn(|index| self.buckets[index].load(Relaxed)),
            sum_nanos: self.sum_nanos.load(Relaxed),
        }
    }
}

#[derive(Clone, Copy, Default)]
struct SendWaitSnapshot {
    buckets: [u64; BUCKET_COUNT],
    sum_nanos: u64,
}

impl SendWaitSnapshot {
    fn append(&self, output: &mut String, direction: Direction) {
        let label = direction.label();
        let mut count = 0;
        for (index, bound_ms) in SEND_WAIT_BUCKETS_MS.iter().enumerate() {
            count += self.buckets[index];
            let bound = *bound_ms as f64 / 1_000.0;
            writeln!(
                output,
                "{SEND_WAIT_SECONDS}_bucket{{direction=\"{label}\",le=\"{bound}\"}} {count}"
            )
            .expect("format histogram bucket into String");
        }
        count += self.buckets[BUCKET_COUNT - 1];
        writeln!(
            output,
            "{SEND_WAIT_SECONDS}_bucket{{direction=\"{label}\",le=\"+Inf\"}} {count}"
        )
        .expect("format overflow bucket into String");
        let sum = self.sum_nanos as f64 / 1_000_000_000.0;
        writeln!(
            output,
            "{SEND_WAIT_SECONDS}_sum{{direction=\"{label}\"}} {sum}"
        )
        .expect("format histogram sum into String");
        writeln!(
            output,
            "{SEND_WAIT_SECONDS}_count{{direction=\"{label}\"}} {count}"
        )
        .expect("format histogram count into String");
    }
}

#[derive(Default)]
struct DirectionCounters {
    frames: AtomicU64,
    bytes: AtomicU64,
    send_wait: SendWait,
}

#[derive(Default)]
pub(super) struct Counters {
    active: AtomicU64,
    closed: [AtomicU64; CloseReason::ALL.len()],
    directions: [DirectionCounters; Direction::ALL.len()],
    published_send_waits: Mutex<[SendWaitSnapshot; Direction::ALL.len()]>,
}

static COUNTERS: std::sync::LazyLock<Counters> = std::sync::LazyLock::new(Counters::default);

/// Publish totals and aggregate send waits on the serving process's diagnostics timer.
pub fn emit_desktop_relays() {
    COUNTERS.emit();
}

/// Append Prometheus histograms from the last diagnostics sample. The
/// recorder buffers raw histogram observations, so the frame path uses fixed
/// atomic storage and the scrape path only formats the published aggregates.
pub fn append_desktop_send_histograms(output: &mut String) {
    COUNTERS.append_send_histograms(output);
}

impl Counters {
    fn emit(&self) {
        metrics::gauge!("cma_desktop_relays_active").set(self.active.load(Relaxed) as f64);
        for reason in CloseReason::ALL {
            metrics::counter!("cma_desktop_relay_closed_total", "reason" => reason.label())
                .absolute(self.closed[reason as usize].load(Relaxed));
        }
        for direction in Direction::ALL {
            let counters = &self.directions[direction as usize];
            metrics::counter!("cma_desktop_relay_frames_total", "direction" => direction.label())
                .absolute(counters.frames.load(Relaxed));
            metrics::counter!("cma_desktop_relay_bytes_total", "direction" => direction.label())
                .absolute(counters.bytes.load(Relaxed));
        }
        let mut published = self
            .published_send_waits
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *published = std::array::from_fn(|index| self.directions[index].send_wait.snapshot());
    }

    fn append_send_histograms(&self, output: &mut String) {
        let published = *self
            .published_send_waits
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        writeln!(output, "# TYPE {SEND_WAIT_SECONDS} histogram")
            .expect("format histogram type into String");
        for direction in Direction::ALL {
            published[direction as usize].append(output, direction);
        }
    }
}

#[derive(Default)]
struct Traffic {
    frames: u64,
    bytes: u64,
    max_send_wait: Duration,
}

pub(super) struct RelayObservation<'a> {
    counters: &'a Counters,
    started: Instant,
    traffic: [Traffic; Direction::ALL.len()],
    authorization_checks: u64,
    max_authorization_wait: Duration,
}

impl RelayObservation<'static> {
    pub(super) fn start() -> Self {
        Self::new(&COUNTERS)
    }
}

impl<'a> RelayObservation<'a> {
    pub(super) fn new(counters: &'a Counters) -> Self {
        counters.active.fetch_add(1, Relaxed);
        Self {
            counters,
            started: Instant::now(),
            traffic: Default::default(),
            authorization_checks: 0,
            max_authorization_wait: Duration::ZERO,
        }
    }

    pub(super) fn forwarded(&mut self, direction: Direction, bytes: usize) {
        let bytes = bytes as u64;
        let index = direction as usize;
        self.traffic[index].frames += 1;
        self.traffic[index].bytes += bytes;
        self.counters.directions[index].frames.fetch_add(1, Relaxed);
        self.counters.directions[index]
            .bytes
            .fetch_add(bytes, Relaxed);
    }

    pub(super) fn send_wait(&mut self, direction: Direction, elapsed: Duration) {
        let index = direction as usize;
        self.traffic[index].max_send_wait = self.traffic[index].max_send_wait.max(elapsed);
        self.counters.directions[index].send_wait.record(elapsed);
    }

    pub(super) fn authorization_check(&mut self, authorized: bool, elapsed: Duration) {
        self.authorization_checks += 1;
        self.max_authorization_wait = self.max_authorization_wait.max(elapsed);
        let outcome = if authorized { "authorized" } else { "refused" };
        metrics::histogram!("cma_desktop_authorization_seconds", "outcome" => outcome)
            .record(elapsed.as_secs_f64());
    }

    pub(super) fn finish(self, reason: CloseReason) {
        let elapsed = self.started.elapsed();
        self.counters.closed[reason as usize].fetch_add(1, Relaxed);
        metrics::histogram!("cma_desktop_relay_lifetime_seconds", "reason" => reason.label())
            .record(elapsed.as_secs_f64());
        let [uplink, downlink] = &self.traffic;
        tracing::info!(
            parent: None,
            reason = reason.label(),
            elapsed_ms = elapsed.as_secs_f64() * 1000.0,
            uplink_frames = uplink.frames,
            uplink_bytes = uplink.bytes,
            uplink_max_send_wait_ms = uplink.max_send_wait.as_secs_f64() * 1000.0,
            downlink_frames = downlink.frames,
            downlink_bytes = downlink.bytes,
            downlink_max_send_wait_ms = downlink.max_send_wait.as_secs_f64() * 1000.0,
            authorization_checks = self.authorization_checks,
            max_authorization_wait_ms = self.max_authorization_wait.as_secs_f64() * 1000.0,
            "desktop relay ended"
        );
    }
}

impl Drop for RelayObservation<'_> {
    fn drop(&mut self) {
        // Cancellation has no observed terminal branch. Only balance the gauge.
        self.counters.active.fetch_sub(1, Relaxed);
    }
}

#[cfg(test)]
#[path = "desktop_telemetry/tests.rs"]
mod tests;
