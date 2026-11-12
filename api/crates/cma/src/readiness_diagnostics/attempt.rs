use super::{micros, record::Record, Recorder};
use std::{
    future::Future,
    sync::{
        atomic::{AtomicU64, AtomicU8, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::time::Instant;

tokio::task_local! { static CHECK: Option<Arc<Progress>>; }
const PHASES: [&str; 7] = [
    "pre_checkout_selection_unknown",
    "checkout",
    "checked_out",
    "command",
    "command_completed",
    "checkout_failed",
    "command_failed",
];

pub(crate) struct Progress {
    pub(crate) recorder: Arc<Recorder>,
    pub(crate) check: u64,
    pub(crate) operation: AtomicU64,
    phase: AtomicU8,
    phase_at: AtomicU64,
}
impl Progress {
    pub(crate) fn current() -> Option<Arc<Self>> {
        CHECK.try_with(Clone::clone).ok().flatten()
    }
    pub(crate) fn event(&self, event: &'static str, operation: u64) {
        let phase = match event {
            "checkout_started" => 1,
            "checked_out" => 2,
            "command_started" => 3,
            "command_succeeded" => 4,
            "checkout_failed" => 5,
            "command_failed" => 6,
            // Span drop can mean cancellation or an unmarked driver result.
            // Preserve the last confirmed phase instead of claiming success.
            "command_completed" => return,
            _ => return,
        };
        if operation != 0 {
            self.operation.store(operation, Ordering::Relaxed);
        }
        self.phase_at
            .store(micros(self.recorder.origin.elapsed()), Ordering::Relaxed);
        self.phase.store(phase, Ordering::Release);
    }
}

pub(crate) struct Attempt {
    progress: Option<Arc<Progress>>,
    started: Instant,
    budget: Duration,
    wake_lag: Duration,
    completed: bool,
}
impl Attempt {
    pub(crate) fn start(
        recorder: Option<Arc<Recorder>>,
        scheduled: Instant,
        budget: Duration,
    ) -> Self {
        let started = Instant::now();
        let progress = recorder.map(|recorder| {
            Arc::new(Progress {
                check: recorder.next_id(),
                operation: AtomicU64::new(0),
                phase: AtomicU8::new(0),
                phase_at: AtomicU64::new(micros(recorder.origin.elapsed())),
                recorder,
            })
        });
        let attempt = Self {
            progress,
            started,
            budget,
            wake_lag: started.saturating_duration_since(scheduled),
            completed: false,
        };
        attempt.emit("started", "pending");
        attempt
    }
    pub(crate) fn check(&self) -> u64 {
        self.progress.as_ref().map_or(0, |p| p.check)
    }
    pub(crate) async fn run<F: Future>(&self, future: F) -> F::Output {
        if self.progress.is_some() {
            CHECK.scope(self.progress.clone(), future).await
        } else {
            future.await
        }
    }
    pub(crate) async fn check_future<F: Future>(
        &self,
        future: F,
    ) -> Result<F::Output, tokio::time::error::Elapsed> {
        if self.progress.is_some() {
            tokio::time::timeout_at(self.started + self.budget, self.run(future)).await
        } else {
            tokio::time::timeout(self.budget, future).await
        }
    }
    fn emit(&self, event: &'static str, outcome: &'static str) {
        let Some(p) = &self.progress else {
            return;
        };
        let stamp = p.recorder.stamp();
        let phase = PHASES[p.phase.load(Ordering::Acquire) as usize];
        let elapsed = self.started.elapsed();
        p.recorder.record(Record::Attempt {
            stamp,
            check: p.check,
            event,
            outcome,
            phase,
            elapsed_us: micros(elapsed),
            budget_us: micros(self.budget),
            deadline_us: micros(self.started.duration_since(p.recorder.origin))
                .saturating_add(micros(self.budget)),
            wake_lag_us: micros(self.wake_lag),
            overshoot_us: if outcome == "deadline" {
                micros(elapsed.saturating_sub(self.budget))
            } else {
                0
            },
            phase_elapsed_us: stamp
                .mono_us
                .saturating_sub(p.phase_at.load(Ordering::Relaxed)),
            operation: p.operation.load(Ordering::Relaxed),
        });
    }
    pub(crate) fn finish(&mut self, outcome: &'static str) {
        self.completed = true;
        self.emit("completed", outcome);
        if let Some(p) = &self.progress {
            metrics::histogram!("cma_readiness_diagnostic_wake_lag_seconds")
                .record(self.wake_lag.as_secs_f64());
            if outcome == "deadline" {
                metrics::histogram!("cma_readiness_diagnostic_deadline_overshoot_seconds").record(
                    self.started
                        .elapsed()
                        .saturating_sub(self.budget)
                        .as_secs_f64(),
                );
                p.recorder.freeze(p.check);
            }
        }
    }
}
impl Drop for Attempt {
    fn drop(&mut self) {
        if !self.completed {
            self.emit("completed", "cancelled");
        }
    }
}

impl Recorder {
    pub(crate) fn state_changed(&self, reason: &'static str, duration_ms: u64) {
        self.record(Record::StateChanged {
            stamp: self.stamp(),
            reason,
            duration_ms,
        });
    }
    pub(crate) fn response(
        &self,
        check: u64,
        reason: &'static str,
        age: Option<Duration>,
        duration_us: u64,
    ) {
        metrics::counter!("cma_readiness_diagnostic_responses_total", "reason" => reason)
            .increment(1);
        // All failed responses; successful totals remain unsampled.
        if reason != "ready" {
            self.record(Record::Response {
                stamp: self.stamp(),
                check,
                reason,
                age_us: micros(age.unwrap_or_default()),
                age_known: age.is_some(),
                check_duration_us: duration_us,
            });
        }
    }
    fn timer(&self, scheduled: Instant) {
        let lag = Instant::now().saturating_duration_since(scheduled);
        metrics::gauge!("cma_readiness_diagnostic_timer_lag_seconds").set(lag.as_secs_f64());
        metrics::histogram!("cma_readiness_diagnostic_timer_lag_duration_seconds")
            .record(lag.as_secs_f64());
        self.record(Record::Timer {
            stamp: self.stamp(),
            lag_us: micros(lag),
        });
    }
}

pub(crate) struct Timer(Option<tokio::task::JoinHandle<()>>);
impl Timer {
    pub(crate) fn start(recorder: Option<&Arc<Recorder>>) -> Self {
        Self(recorder.map(|r| {
            let weak = Arc::downgrade(r);
            tokio::spawn(async move {
                loop {
                    let scheduled = Instant::now() + Duration::from_secs(1);
                    tokio::time::sleep_until(scheduled).await;
                    let Some(recorder) = weak.upgrade() else {
                        break;
                    };
                    recorder.timer(scheduled);
                }
            })
        }))
    }
}
impl Drop for Timer {
    fn drop(&mut self) {
        if let Some(task) = &self.0 {
            task.abort();
        }
    }
}
