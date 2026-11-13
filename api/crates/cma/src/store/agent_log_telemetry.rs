//! Complete-scan accounting on the shared periodic read-telemetry path.
use super::cooperative_budget::{CooperativeBudget, Policy};
use super::execution_sections::{Clock, MonotonicClock};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, LazyLock,
};
use std::time::Instant;

mod caller;
pub(super) use caller::ScanCaller;
use caller::CALLERS;

#[derive(Default)]
struct Totals {
    scans: AtomicU64,
    documents: AtomicU64,
    bytes: AtomicU64,
    duration_us: AtomicU64,
    changed: AtomicBool,
}
struct Counters([[Totals; 2]; CALLERS.len()]);
impl Default for Counters {
    fn default() -> Self {
        Self(std::array::from_fn(|_| {
            std::array::from_fn(|_| Totals::default())
        }))
    }
}
static COUNTERS: LazyLock<Arc<Counters>> = LazyLock::new(Arc::default);
fn counters() -> Arc<Counters> {
    #[cfg(test)]
    if let Ok(counters) = TEST_COUNTERS.try_with(Arc::clone) {
        return counters;
    }
    Arc::clone(&COUNTERS)
}

/// One complete validation attempt. No atomics or log events per document;
/// Drop retains consumed work and elapsed wall time on failure/cancellation.
pub(super) struct Scan<C: Clock = MonotonicClock> {
    counters: Arc<Counters>,
    caller: ScanCaller,
    started: Instant,
    documents: u64,
    bytes: u64,
    success: bool,
    execution: CooperativeBudget<C>,
}
impl Scan {
    pub(super) fn new(caller: ScanCaller) -> Self {
        Self::with_counters(counters(), caller)
    }
    fn with_counters(counters: Arc<Counters>, caller: ScanCaller) -> Self {
        Self::from_parts(counters, caller, Policy::default(), MonotonicClock)
    }
}
impl<C: Clock> Scan<C> {
    pub(super) fn with_clock(caller: ScanCaller, policy: Policy, clock: C) -> Self {
        Self::from_parts(counters(), caller, policy, clock)
    }
    fn from_parts(counters: Arc<Counters>, caller: ScanCaller, policy: Policy, clock: C) -> Self {
        Self {
            counters,
            caller,
            started: Instant::now(),
            documents: 0,
            bytes: 0,
            success: false,
            execution: CooperativeBudget::new(policy, clock),
        }
    }
    pub(super) fn observe<'a, F>(
        &'a mut self,
        future: F,
        record_poll: bool,
    ) -> impl std::future::Future<Output = F::Output> + Send + 'a
    where
        F: std::future::Future + Send + 'a,
    {
        self.execution.observe(future, record_poll)
    }
    pub(super) async fn checkpoint(&mut self) {
        self.execution.checkpoint().await;
    }
    pub(super) fn document(&mut self, encoded_bytes: usize) {
        self.documents = self.documents.saturating_add(1);
        self.bytes = self.bytes.saturating_add(encoded_bytes as u64);
    }
    pub(super) fn complete(&mut self) {
        self.success = true;
    }
}
impl<C: Clock> Drop for Scan<C> {
    fn drop(&mut self) {
        let (execution, yields) = self.execution.finish();
        #[cfg(test)]
        if let Ok(samples) = TEST_EXECUTION.try_with(Arc::clone) {
            samples.lock().unwrap().push((execution, yields));
        }
        metrics::histogram!("cma_agent_log_scan_longest_section_seconds", "caller" => self.caller.name())
            .record(execution.longest_section.as_secs_f64());
        metrics::histogram!("cma_agent_log_scan_driver_poll_seconds", "caller" => self.caller.name())
            .record(execution.longest_poll.as_secs_f64());
        metrics::counter!("cma_agent_log_scan_yields_total", "caller" => self.caller.name())
            .increment(yields);
        tracing::debug!(parent: None, caller = self.caller.name(), documents = self.documents, bytes = self.bytes,
            outcome = if self.success { "success" } else { "failed" }, sections = execution.sections, cooperative_yields = yields,
            longest_section_seconds = execution.longest_section.as_secs_f64(),
            longest_driver_poll_seconds = execution.longest_poll.as_secs_f64(),
            "Agent log scan execution sections");
        let totals = &self.counters.0[self.caller.index()][usize::from(self.success)];
        add(&totals.scans, 1);
        add(&totals.documents, self.documents);
        add(&totals.bytes, self.bytes);
        add(
            &totals.duration_us,
            self.started.elapsed().as_micros().min(u64::MAX as u128) as u64,
        );
        totals.changed.store(true, Ordering::Release);
    }
}
fn add(counter: &AtomicU64, delta: u64) {
    if delta != 0 {
        let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
            Some(v.saturating_add(delta))
        });
    }
}
pub(crate) fn emit() {
    super::agent_log_integrity::fleet::telemetry::emit();
    counters().emit();
}
impl Counters {
    fn emit(&self) {
        for (classes, caller) in self.0.iter().zip(CALLERS) {
            for (totals, outcome) in classes.iter().zip(["failed", "success"]) {
                if !totals.changed.swap(false, Ordering::Acquire) {
                    continue;
                }
                for (metric, counter) in [
                    ("cma_agent_log_scans_total", &totals.scans),
                    ("cma_agent_log_scan_documents_total", &totals.documents),
                    ("cma_agent_log_scan_bytes_total", &totals.bytes),
                    (
                        "cma_agent_log_scan_duration_microseconds_total",
                        &totals.duration_us,
                    ),
                ] {
                    metrics::counter!(metric, "caller" => caller.name(), "worker_step" => caller.worker_step(), "outcome" => outcome).absolute(counter.load(Ordering::Relaxed));
                    tracing::info!(parent: None, metric, caller = caller.name(),
                        worker_step = caller.worker_step(), outcome,
                        value = counter.load(Ordering::Relaxed), "complete Agent log scan counter");
                }
            }
        }
    }
}

#[cfg(test)]
tokio::task_local! { static TEST_COUNTERS: Arc<Counters>; }
#[cfg(test)]
pub(in crate::store) async fn isolated_test<F: std::future::Future>(future: F) -> F::Output {
    TEST_COUNTERS.scope(Arc::default(), future).await
}
#[cfg(test)]
pub(in crate::store) fn snapshot(caller: ScanCaller, success: bool) -> [u64; 4] {
    let counters = counters();
    let totals = &counters.0[caller.index()][usize::from(success)];
    [
        &totals.scans,
        &totals.documents,
        &totals.bytes,
        &totals.duration_us,
    ]
    .map(|v| v.load(Ordering::Relaxed))
}
#[cfg(test)]
pub(in crate::store) fn snapshot_all() -> [u64; 4] {
    let mut total = [0; 4];
    for caller in CALLERS {
        for success in [false, true] {
            for (sum, value) in total.iter_mut().zip(snapshot(caller, success)) {
                *sum += value;
            }
        }
    }
    total
}
#[cfg(test)]
pub(crate) fn record_all_execution_callers_for_test() {
    let counters = Arc::new(Counters::default());
    for caller in CALLERS {
        for complete in [false, true] {
            let mut scan = Scan::with_counters(Arc::clone(&counters), caller);
            scan.document(1);
            if complete {
                scan.complete();
            }
        }
    }
}
#[cfg(test)]
#[path = "agent_log_telemetry_tests.rs"]
mod tests;

#[cfg(test)]
tokio::task_local! {
    static TEST_EXECUTION: Arc<std::sync::Mutex<Vec<(super::execution_sections::SectionStats, u64)>>>;
}
#[cfg(test)]
pub(in crate::store) async fn recorded_execution<F: std::future::Future>(
    future: F,
) -> (
    F::Output,
    Vec<(super::execution_sections::SectionStats, u64)>,
) {
    let samples = Arc::new(std::sync::Mutex::new(Vec::new()));
    let output = TEST_EXECUTION.scope(samples.clone(), future).await;
    let measured = samples.lock().unwrap().clone();
    (output, measured)
}
