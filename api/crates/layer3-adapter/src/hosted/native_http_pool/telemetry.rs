//! Pool-local RAII accounting. Labels describe fixed transport roles, never a
//! runtime. Deltas preserve independent pool contributions and cloned handles.
//! Slots include allocations retained by cancelled callers until their last
//! handle drops. Cached connections count retained idle senders, not a liveness
//! probe. Busy counts exclusive checkouts, including connection setup; waiting
//! counts pending connection-lock acquisitions. Neither counts request permits.
//! The production composition has one native pool per process. The pools gauge
//! exposes additional same-role pools instead of silently overwriting them.
use super::{NativeHttpLane, RuntimeWorkloadClass, MAX_HTTP_SESSIONS};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::hosted::provider_transport) enum Gauge {
    Pools,
    Capacity,
    Reserve,
    Slots,
    CachedConnections,
    Busy,
    Waiting,
}
impl Gauge {
    fn label(self) -> &'static str {
        match self {
            Self::Pools => "pools",
            Self::Capacity => "capacity",
            Self::Reserve => "reserve",
            Self::Slots => "slots",
            Self::CachedConnections => "cached_connections",
            Self::Busy => "busy",
            Self::Waiting => "waiting",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::hosted::provider_transport) enum Outcome {
    Fresh,
    Reused,
    Retired,
    Cancelled,
}
impl Outcome {
    fn label(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Reused => "reused",
            Self::Retired => "retired",
            Self::Cancelled => "cancelled",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::hosted::provider_transport) enum Refusal {
    Reserve,
    Exhausted,
}
#[derive(Clone, Copy, Debug)]
pub(in crate::hosted::provider_transport) enum Event {
    Gauge(Gauge, i64),
    Checkout(NativeHttpLane, Outcome, Duration),
    Refused(NativeHttpLane, RuntimeWorkloadClass, Refusal),
}
pub(in crate::hosted::provider_transport) trait Observer:
    Send + Sync
{
    fn record(&self, event: Event);
}

struct Metrics {
    role: &'static str,
    gauges: [metrics::Gauge; 7],
    checkouts: [[metrics::Counter; 4]; 3],
    waits: [[metrics::Histogram; 4]; 3],
    refusals: [[[metrics::Counter; 2]; 3]; 3],
}
impl Metrics {
    fn new(role: &'static str) -> Self {
        let gauges = [Gauge::Pools, Gauge::Capacity, Gauge::Reserve, Gauge::Slots,
            Gauge::CachedConnections, Gauge::Busy, Gauge::Waiting].map(|kind| {
            let gauge = metrics::gauge!("cma_adapter_native_http_pool", "pool" => role, "kind" => kind.label());
            gauge.increment(0.0);
            gauge
        });
        let lanes = ["close_agent", "control", "ownership"];
        let outcomes = [
            Outcome::Fresh,
            Outcome::Reused,
            Outcome::Retired,
            Outcome::Cancelled,
        ];
        Self {
            role,
            gauges,
            checkouts: lanes.map(|lane| outcomes.map(|outcome| metrics::counter!(
                "cma_adapter_native_http_checkouts_total", "pool" => role, "lane" => lane, "outcome" => outcome.label()))),
            waits: lanes.map(|lane| outcomes.map(|outcome| metrics::histogram!(
                "cma_adapter_native_http_checkout_wait_seconds", "pool" => role, "lane" => lane, "outcome" => outcome.label()))),
            refusals: lanes.map(|lane| ["ordinary", "continuity", "control"].map(|class| ["reserve", "exhausted"].map(|reason| metrics::counter!(
                "cma_adapter_native_http_refusals_total", "pool" => role, "lane" => lane, "class" => class, "reason" => reason)))),
        }
    }
}
impl Observer for Metrics {
    fn record(&self, event: Event) {
        match event {
            Event::Gauge(kind, delta) => self.gauges[kind as usize].increment(delta as f64),
            Event::Checkout(lane, outcome, elapsed) => {
                self.checkouts[lane as usize][outcome as usize].increment(1);
                self.waits[lane as usize][outcome as usize].record(elapsed.as_secs_f64());
            }
            Event::Refused(lane, class, reason) => {
                self.refusals[lane as usize][class as usize][reason as usize].increment(1);
                tracing::debug!(
                    pool = self.role,
                    ?lane,
                    ?class,
                    ?reason,
                    "Native HTTP pool refused admission"
                );
            }
        }
    }
}

pub(in crate::hosted::provider_transport) struct Pool {
    observer: Arc<dyn Observer>,
    reserve: usize,
}
impl Pool {
    pub(super) fn metrics(native: bool, reserve: usize) -> Arc<Self> {
        Self::new(
            Arc::new(Metrics::new(if native {
                "native"
            } else {
                "retained_provider"
            })),
            reserve,
        )
    }
    pub(in crate::hosted::provider_transport) fn new(
        observer: Arc<dyn Observer>,
        reserve: usize,
    ) -> Arc<Self> {
        observer.record(Event::Gauge(Gauge::Pools, 1));
        observer.record(Event::Gauge(Gauge::Capacity, MAX_HTTP_SESSIONS as i64));
        observer.record(Event::Gauge(Gauge::Reserve, reserve as i64));
        Arc::new(Self { observer, reserve })
    }
    pub(super) fn guard(self: &Arc<Self>, kind: Gauge) -> Guard {
        self.observer.record(Event::Gauge(kind, 1));
        Guard {
            pool: self.clone(),
            kind,
        }
    }
    pub(super) fn waiting(self: &Arc<Self>, lane: NativeHttpLane) -> Checkout {
        Checkout {
            waiting: Some(self.guard(Gauge::Waiting)),
            pool: self.clone(),
            lane,
            started: Instant::now(),
            elapsed: None,
            outcome: Outcome::Cancelled,
        }
    }
    pub(super) fn refused(
        &self,
        lane: NativeHttpLane,
        class: RuntimeWorkloadClass,
        reason: Refusal,
    ) {
        self.observer.record(Event::Refused(lane, class, reason));
    }
}
impl Drop for Pool {
    fn drop(&mut self) {
        self.observer.record(Event::Gauge(Gauge::Pools, -1));
        self.observer
            .record(Event::Gauge(Gauge::Capacity, -(MAX_HTTP_SESSIONS as i64)));
        self.observer
            .record(Event::Gauge(Gauge::Reserve, -(self.reserve as i64)));
    }
}
pub(super) struct Guard {
    pool: Arc<Pool>,
    kind: Gauge,
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.pool.observer.record(Event::Gauge(self.kind, -1));
    }
}
pub(super) struct Checkout {
    waiting: Option<Guard>,
    pool: Arc<Pool>,
    lane: NativeHttpLane,
    started: Instant,
    elapsed: Option<Duration>,
    outcome: Outcome,
}
impl Checkout {
    pub(super) fn acquired(&mut self) -> Guard {
        self.elapsed = Some(self.started.elapsed());
        self.waiting.take();
        self.pool.guard(Gauge::Busy)
    }
    pub(super) fn finish(&mut self, outcome: Outcome) {
        self.outcome = outcome;
    }
}
impl Drop for Checkout {
    fn drop(&mut self) {
        self.pool.observer.record(Event::Checkout(
            self.lane,
            self.outcome,
            self.elapsed.unwrap_or_else(|| self.started.elapsed()),
        ));
    }
}
