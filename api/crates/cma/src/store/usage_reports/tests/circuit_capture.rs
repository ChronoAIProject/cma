//! Capture only fixed circuit fields, never Mongo commands, names, or query text.
use super::*;
use std::sync::{atomic::Ordering, Mutex};
use std::time::Instant;
use tracing::{field::Visit, Subscriber};
use tracing_subscriber::{layer::Context, prelude::*, Layer};

#[derive(Clone, Debug, Default)]
pub(crate) struct CircuitEvent {
    pub reason: String,
    pub elapsed_ms: u64,
    pub failed: bool,
    pub probe: bool,
    pub operation: String,
    pub client_key: u64,
    pub connection_key: u64,
    pub observed_phase: EventPhase,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct EventPhase {
    workload: &'static str,
    cohort_size: usize,
    phase: &'static str,
    page: usize,
}
impl Visit for CircuitEvent {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "operation" {
            self.operation = value.into();
        }
        if field.name() == "reason" {
            self.reason = value.into();
        }
    }
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        if field.name() == "client_key" {
            self.client_key = value;
        }
        if field.name() == "connection_key" {
            self.connection_key = value;
        }
        if field.name() == "elapsed_ms" {
            self.elapsed_ms = value;
        }
    }
    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        if field.name() == "probe" {
            self.probe = value;
        }
        if field.name() == "failed" {
            self.failed = value;
        }
    }
    fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
}
#[derive(Clone, Default)]
pub(crate) struct Capture(
    Arc<Mutex<(usize, Option<CircuitEvent>)>>,
    Arc<Mutex<EventPhase>>,
);
impl Capture {
    pub fn snapshot(&self) -> (usize, Option<CircuitEvent>) {
        self.0.lock().unwrap().clone()
    }
    fn phase(&self, phase: EventPhase) {
        *self.1.lock().unwrap() = phase;
    }
}
impl<S: Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        if !event
            .metadata()
            .target()
            .ends_with("::usage_reports::budget")
        {
            return;
        }
        let mut fields = CircuitEvent::default();
        event.record(&mut fields);
        if fields.reason.is_empty() {
            return;
        }
        // Snapshot when the event arrives, rather than assigning a later
        // refused query's phase to a circuit opened during fixture setup.
        fields.observed_phase = self.1.lock().unwrap().clone();
        let phase = &fields.observed_phase;
        eprintln!(
            "metrics_scale workload={} cohort_size={} event_phase={} event_page={} circuit_reason={} event_elapsed_ms={} checkout_failed={} probe={} operation={} client_key={} connection_key={}",
            phase.workload, phase.cohort_size, phase.phase, phase.page,
            fields.reason, fields.elapsed_ms, fields.failed, fields.probe, fields.operation, fields.client_key, fields.connection_key,
        );
        if fields.probe {
            return;
        }
        let mut state = self.0.lock().unwrap();
        state.0 += 1;
        state.1 = Some(fields);
    }
}

pub(super) struct ScaleWindow {
    workload: &'static str,
    cohort_size: usize,
    started: Instant,
    phase_started: Instant,
    phase: &'static str,
    page: usize,
    capture: Capture,
    circuit: Option<Arc<std::sync::atomic::AtomicU64>>,
}
impl ScaleWindow {
    pub fn start() -> Self {
        Self::start_for("report", 20_001)
    }
    pub fn start_for(workload: &'static str, cohort_size: usize) -> Self {
        let capture = Capture::default();
        capture.phase(EventPhase {
            workload,
            cohort_size,
            phase: "connect",
            page: 0,
        });
        // Nextest runs this ignored test in its own process. The global subscriber
        // also observes driver tasks, which a thread-local subscriber would miss.
        tracing::subscriber::set_global_default(
            tracing_subscriber::registry().with(
                capture.clone().with_filter(
                    tracing_subscriber::filter::Targets::new()
                        .with_target("cma::store::usage_reports::budget", tracing::Level::INFO),
                ),
            ),
        )
        .expect("scale contract requires an isolated test process");
        Self {
            workload,
            cohort_size,
            started: Instant::now(),
            phase_started: Instant::now(),
            phase: "connect",
            page: 0,
            capture,
            circuit: None,
        }
    }
    pub fn attach(&mut self, store: &ReportingStore) {
        self.circuit = Some(store.circuit.clone());
    }
    pub fn cohort(&mut self, count: usize) {
        self.cohort_size = count;
        self.circuit = None;
        self.phase("connect", 0);
    }
    pub fn phase(&mut self, phase: &'static str, page: usize) {
        eprintln!(
            "metrics_phase workload={} cohort_size={} phase={} page={} elapsed_ms={}",
            self.workload,
            self.cohort_size,
            self.phase,
            self.page,
            self.phase_started.elapsed().as_millis()
        );
        self.phase = phase;
        self.page = page;
        self.phase_started = Instant::now();
        self.capture.phase(EventPhase {
            workload: self.workload,
            cohort_size: self.cohort_size,
            phase,
            page,
        });
    }
    pub fn assert_no_pressure(&self) {
        assert_eq!(
            self.capture.snapshot().0,
            0,
            "isolated supported workload opened the reporting circuit"
        );
    }
}
impl Drop for ScaleWindow {
    fn drop(&mut self) {
        let (opens, last) = self.capture.snapshot();
        let remaining = self
            .circuit
            .as_ref()
            .map_or(0, |c| c.load(Ordering::Relaxed).saturating_sub(now()));
        eprintln!(
            "metrics_scale workload={} cohort_size={} result={} phase={} page={} elapsed_ms={} phase_elapsed_ms={} circuit_opens={} circuit_remaining_seconds={} last_circuit_event={:?}",
            self.workload, self.cohort_size,
            if std::thread::panicking() { "failed" } else { "passed" },
            self.phase, self.page, self.started.elapsed().as_millis(),
            self.phase_started.elapsed().as_millis(), opens, remaining, last
        );
    }
}

#[test]
fn opening_event_keeps_setup_phase_when_later_option_page_is_refused() {
    let capture = Capture::default();
    let setup = EventPhase {
        workload: "options",
        cohort_size: 20_001,
        phase: "seed_inventory",
        page: 0,
    };
    capture.phase(setup.clone());
    tracing::subscriber::with_default(tracing_subscriber::registry().with(capture.clone()), || {
        tracing::warn!(target: "cma::store::usage_reports::budget",
            reason = "connection_hold_deadline", elapsed_ms = 2001_u64, failed = false)
    });
    capture.phase(EventPhase {
        phase: "owner_page",
        page: 2,
        ..setup.clone()
    });
    let (count, event) = capture.snapshot();
    assert_eq!(count, 1);
    let event = event.unwrap();
    assert_eq!(event.observed_phase, setup);
    assert_eq!(event.reason, "connection_hold_deadline");
    assert_eq!(event.elapsed_ms, 2001);
    assert!(!event.failed);
}

#[test]
fn owner_opening_keeps_setup_or_read_page_when_a_later_page_is_refused() {
    for (phase, page) in [("seed_report_chunk", 101), ("owner_page", 42)] {
        let capture = Capture::default();
        let opening = EventPhase {
            workload: "owner_allocation",
            cohort_size: 20_001,
            phase,
            page,
        };
        capture.phase(opening.clone());
        tracing::subscriber::with_default(
            tracing_subscriber::registry().with(capture.clone()),
            || {
                tracing::warn!(target: "cma::store::usage_reports::budget",
                reason = "connection_hold_deadline", elapsed_ms = 2001_u64, failed = false);
            },
        );
        capture.phase(EventPhase {
            phase: "owner_page",
            page: 43,
            ..opening.clone()
        });
        let (count, event) = capture.snapshot();
        assert_eq!(count, 1);
        let event = event.unwrap();
        assert_eq!(event.observed_phase, opening);
        assert_eq!(event.reason, "connection_hold_deadline");
        assert_eq!(event.elapsed_ms, 2001);
        assert!(!event.failed);
    }
}

#[test]
fn timing_probes_neither_count_as_openings_nor_replace_the_last_opening() {
    let capture = Capture::default();
    let opening = EventPhase {
        workload: "owner_allocation",
        cohort_size: 20_001,
        phase: "owner_page",
        page: 42,
    };
    capture.phase(opening.clone());
    tracing::subscriber::with_default(tracing_subscriber::registry().with(capture.clone()), || {
        tracing::info!(target: "cma::store::usage_reports::budget",
            reason = "checkout_sample", probe = true, elapsed_ms = 0_u64,
            client_key = 3_u64, connection_key = 7_u64, failed = false);
        assert_eq!(capture.snapshot().0, 0);
        assert!(capture.snapshot().1.is_none());
        tracing::warn!(target: "cma::store::usage_reports::budget",
            reason = "connection_hold_deadline", elapsed_ms = 2001_u64, failed = false);
        capture.phase(EventPhase {
            page: 43,
            ..opening.clone()
        });
        for reason in [
            "connection_hold_sample",
            "owner_aggregate_sample",
            "owner_labels_sample",
            "read_wall_sample",
        ] {
            tracing::info!(target: "cma::store::usage_reports::budget",
                reason, probe = true, elapsed_ms = 2001_u64,
                client_key = 3_u64, connection_key = 7_u64, operation = "owner_allocations");
        }
    });
    let (count, event) = capture.snapshot();
    assert_eq!(count, 1);
    let event = event.unwrap();
    assert_eq!(event.observed_phase, opening);
    assert_eq!(event.reason, "connection_hold_deadline");
    assert_eq!(event.elapsed_ms, 2001);
    assert!(!event.probe);
}
