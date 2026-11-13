use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};

use layer2_core::sandbox::store::SandboxStoreError;
use tokio::{sync::OwnedSemaphorePermit, time::Instant};

use super::BackgroundBudget;

const BUSY_DELAY: Duration = Duration::from_millis(250);
const MAX_RETRY: Duration = Duration::from_secs(60);

struct Progress {
    active: bool,
    next: Instant,
    failures: u32,
    pending_since: Option<Instant>,
    completed: u64,
}

#[derive(Clone)]
pub(crate) struct BackgroundWork {
    budget: Arc<BackgroundBudget>,
    name: &'static str,
    deadline: Duration,
    interval: Duration,
    progress: Arc<Mutex<Progress>>,
}

impl BackgroundWork {
    pub(crate) fn new(
        budget: Arc<BackgroundBudget>,
        name: &'static str,
        deadline: Duration,
        interval: Duration,
    ) -> Self {
        Self {
            budget,
            name,
            deadline,
            interval,
            progress: Arc::new(Mutex::new(Progress {
                active: false,
                next: Instant::now(),
                failures: 0,
                pending_since: None,
                completed: 0,
            })),
        }
    }

    /// A deferred attempt is not polled and never enters the driver's queue.
    /// Callers retain durable cursors/identities and retry the same work later.
    pub(crate) async fn run<T, F>(
        &self,
        operation: impl FnOnce() -> F,
    ) -> Result<Option<T>, SandboxStoreError>
    where
        F: Future<Output = Result<T, SandboxStoreError>>,
    {
        self.run_with(operation, |_| true).await
    }

    /// `false` yields a resumable sweep without counting a completion or failure.
    pub(crate) async fn run_resumable<F>(
        &self,
        operation: impl FnOnce() -> F,
    ) -> Result<Option<bool>, SandboxStoreError>
    where
        F: Future<Output = Result<bool, SandboxStoreError>>,
    {
        self.run_with(operation, |complete| *complete).await
    }

    async fn run_with<T, F>(
        &self,
        operation: impl FnOnce() -> F,
        complete: impl FnOnce(&T) -> bool,
    ) -> Result<Option<T>, SandboxStoreError>
    where
        F: Future<Output = Result<T, SandboxStoreError>>,
    {
        let next = self.progress.lock().unwrap().next;
        tokio::time::sleep_until(next).await;
        let Some(mut attempt) = self.try_start() else {
            return Ok(None);
        };
        // Dropping the timeout releases the operation before its budget permit.
        // Callers must be replay-safe: a cancelled write is never proof of failure.
        let result = tokio::time::timeout(
            self.deadline,
            layer2_core::diagnostic_context::DiagnosticContext::background(self.name)
                .scope(operation()),
        )
        .await;
        let reason = match &result {
            Ok(Ok(value)) => {
                if complete(value) {
                    "complete"
                } else {
                    "yielded"
                }
            }
            Ok(Err(_)) => "unavailable",
            Err(_) => "deadline",
        };
        attempt.finish(reason);
        match result {
            Ok(result) => result.map(Some),
            Err(_) => Err(SandboxStoreError::Unavailable(format!(
                "{} background deadline exceeded",
                self.name
            ))),
        }
    }

    pub(crate) fn foreground(&self) -> super::Foreground {
        self.budget.foreground()
    }

    pub(crate) fn next_run_at(&self) -> Instant {
        self.progress.lock().unwrap().next
    }

    fn try_start(&self) -> Option<Attempt> {
        let now = Instant::now();
        let mut progress = self.progress.lock().unwrap();
        if progress.active || now < progress.next {
            return None;
        }
        progress.pending_since.get_or_insert(now);
        // A timestamp lets the evaluator detect growing age while an attempt
        // is blocked; the sampled value does not need a running worker timer.
        let pending =
            crate::telemetry::timestamp() - progress.pending_since.unwrap().elapsed().as_secs_f64();
        metrics::gauge!("cma_background_pending_since_timestamp_seconds", "work" => self.name)
            .set(pending);
        let Some(permit) = self.budget.try_enter() else {
            progress.next = now + jitter(BUSY_DELAY);
            metrics::counter!("cma_background_deferred_total", "work" => self.name).increment(1);
            tracing::info!(parent: None,
                work = self.name, reason = "shared_budget_denied",
                attempted = 0_u64, admitted = 0_u64, completed = 0_u64,
                pending_ms = progress.pending_since.unwrap().elapsed().as_millis() as u64,
                "background work deferred by shared budget"
            );
            return None;
        };
        progress.active = true;
        metrics::gauge!("cma_background_active", "work" => self.name).set(1);
        Some(Attempt {
            work: self.clone(),
            started: now,
            finished: false,
            _permit: permit,
        })
    }
}

struct Attempt {
    work: BackgroundWork,
    started: Instant,
    finished: bool,
    _permit: OwnedSemaphorePermit,
}

impl Attempt {
    fn finish(&mut self, reason: &'static str) {
        let mut progress = self.work.progress.lock().unwrap();
        let pending_ms = progress
            .pending_since
            .map_or(0, |since| since.elapsed().as_millis() as u64);
        let delay = if reason == "complete" {
            progress.failures = 0;
            progress.pending_since = None;
            progress.completed += 1;
            self.work.interval
        } else if reason == "yielded" {
            progress.failures = 0;
            BUSY_DELAY
        } else {
            progress.failures = (progress.failures + 1).min(6);
            Duration::from_secs(1 << progress.failures).min(MAX_RETRY)
        };
        progress.next = Instant::now() + jitter(delay);
        progress.active = false;
        metrics::gauge!("cma_background_active", "work" => self.work.name).set(0);
        metrics::counter!("cma_background_attempts_total", "work" => self.work.name, "outcome" => reason).increment(1);
        metrics::histogram!("cma_background_attempt_seconds", "work" => self.work.name, "outcome" => reason).record(self.started.elapsed().as_secs_f64());
        if reason == "complete" {
            metrics::gauge!("cma_background_pending_since_timestamp_seconds", "work" => self.work.name).set(0);
            metrics::gauge!("cma_background_completion_timestamp_seconds", "work" => self.work.name).set(crate::telemetry::timestamp());
        }
        self.finished = true;
        tracing::info!(
            work = self.work.name,
            reason,
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            pending_ms,
            completed = progress.completed,
            retry_ms = progress
                .next
                .saturating_duration_since(Instant::now())
                .as_millis() as u64,
            "background work attempt completed"
        );
    }
}

impl Drop for Attempt {
    fn drop(&mut self) {
        if !self.finished {
            self.finish("cancelled");
        }
    }
}

// Equal jitter in [75%, 100%] keeps the bound explicit and avoids synchronized
// retry bursts across workers/processes. UUID randomness is already available.
fn jitter(delay: Duration) -> Duration {
    let millis = delay.as_millis() as u64;
    if millis == 0 {
        return Duration::ZERO;
    }
    let spread = millis / 4;
    let random = uuid::Uuid::new_v4().as_u128() as u64;
    Duration::from_millis(millis - spread + random % (spread + 1))
}
