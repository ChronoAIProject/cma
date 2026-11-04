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
        let next = self.progress.lock().unwrap().next;
        tokio::time::sleep_until(next).await;
        let Some(mut attempt) = self.try_start() else {
            return Ok(None);
        };
        // Dropping the timeout releases the operation before its budget permit.
        // Callers must be replay-safe: a cancelled write is never proof of failure.
        let result = tokio::time::timeout(self.deadline, operation()).await;
        let reason = match &result {
            Ok(Ok(_)) => "complete",
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

    fn try_start(&self) -> Option<Attempt> {
        let now = Instant::now();
        let mut progress = self.progress.lock().unwrap();
        if progress.active || now < progress.next {
            return None;
        }
        progress.pending_since.get_or_insert(now);
        let Some(permit) = self.budget.try_enter() else {
            progress.next = now + jitter(BUSY_DELAY);
            tracing::debug!(
                work = self.name,
                pending_ms = progress.pending_since.unwrap().elapsed().as_millis() as u64,
                "background work deferred by shared budget"
            );
            return None;
        };
        progress.active = true;
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
        } else {
            progress.failures = (progress.failures + 1).min(6);
            Duration::from_secs(1 << progress.failures).min(MAX_RETRY)
        };
        progress.next = Instant::now() + jitter(delay);
        progress.active = false;
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
