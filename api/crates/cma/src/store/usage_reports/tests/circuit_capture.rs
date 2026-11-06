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
}
impl Visit for CircuitEvent {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "reason" {
            self.reason = value.into();
        }
    }
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        if field.name() == "elapsed_ms" {
            self.elapsed_ms = value;
        }
    }
    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        if field.name() == "failed" {
            self.failed = value;
        }
    }
    fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
}
#[derive(Clone, Default)]
pub(crate) struct Capture(Arc<Mutex<(usize, Option<CircuitEvent>)>>);
impl Capture {
    pub fn snapshot(&self) -> (usize, Option<CircuitEvent>) {
        self.0.lock().unwrap().clone()
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
        eprintln!(
            "metrics_scale circuit_reason={} event_elapsed_ms={} checkout_failed={}",
            fields.reason, fields.elapsed_ms, fields.failed
        );
        let mut state = self.0.lock().unwrap();
        state.0 += 1;
        state.1 = Some(fields);
    }
}

pub(super) struct ScaleWindow {
    started: Instant,
    phase_started: Instant,
    phase: &'static str,
    page: usize,
    capture: Capture,
    circuit: Option<Arc<std::sync::atomic::AtomicU64>>,
}
impl ScaleWindow {
    pub fn start() -> Self {
        let capture = Capture::default();
        // Nextest runs this ignored test in its own process. The global subscriber
        // also observes driver tasks, which a thread-local subscriber would miss.
        tracing::subscriber::set_global_default(
            tracing_subscriber::registry().with(
                capture.clone().with_filter(
                    tracing_subscriber::filter::Targets::new()
                        .with_target("cma::store::usage_reports::budget", tracing::Level::WARN),
                ),
            ),
        )
        .expect("scale contract requires an isolated test process");
        Self {
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
    pub fn phase(&mut self, phase: &'static str, page: usize) {
        self.phase = phase;
        self.page = page;
        self.phase_started = Instant::now();
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
            "metrics_scale result={} phase={} page={} elapsed_ms={} phase_elapsed_ms={} circuit_opens={} circuit_remaining_seconds={} last_circuit_event={:?}",
            if std::thread::panicking() { "failed" } else { "passed" },
            self.phase, self.page, self.started.elapsed().as_millis(),
            self.phase_started.elapsed().as_millis(), opens, remaining, last
        );
    }
}
