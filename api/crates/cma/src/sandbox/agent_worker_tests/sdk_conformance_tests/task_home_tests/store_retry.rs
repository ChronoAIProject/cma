use std::panic::Location;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::sandbox::lifecycle_failure::LifecycleTaskFailure;
use layer2_core::sandbox::store::SandboxStoreError;

mod snapshot;
#[cfg(test)]
mod tests;

pub(super) struct StoreRetry {
    desktop: bool,
    started: tokio::time::Instant,
    unavailable_retries: AtomicUsize,
    conflicts: AtomicUsize,
    last_failure: std::sync::Mutex<Option<String>>,
}

impl StoreRetry {
    pub(super) fn new(desktop: bool) -> Self {
        Self {
            desktop,
            started: tokio::time::Instant::now(),
            unavailable_retries: AtomicUsize::new(0),
            conflicts: AtomicUsize::new(0),
            last_failure: std::sync::Mutex::new(None),
        }
    }

    #[track_caller]
    pub(super) fn agent<'a>(
        &'a self,
        harness: &'a Harness<MongoSandboxStore>,
        id: &'a AgentId,
    ) -> impl std::future::Future<Output = Option<bool>> + 'a {
        let caller = Location::caller();
        async move {
            let result = harness
                .worker
                .run_one_diagnostic(id, SystemTime::now())
                .await;
            self.complete(result, "Agent", caller, snapshot::agent(harness, id))
                .await
        }
    }

    #[track_caller]
    pub(super) fn sandbox<'a>(
        &'a self,
        harness: &'a Harness<MongoSandboxStore>,
        worker: &'a crate::sandbox::worker::SandboxWorker,
        id: &'a SandboxId,
    ) -> impl std::future::Future<Output = Option<bool>> + 'a {
        let caller = Location::caller();
        async move {
            let result = worker.run_one_diagnostic(id, SystemTime::now()).await;
            self.complete(result, "Sandbox", caller, snapshot::sandbox(harness, id))
                .await
        }
    }

    async fn complete<T>(
        &self,
        result: Result<T, LifecycleTaskFailure>,
        worker: &str,
        caller: &Location<'_>,
        snapshot: impl std::future::Future<Output = String>,
    ) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(failure) => {
                let recorded = self.record(failure, worker, caller);
                let state = snapshot.await;
                recorded.finish(state)
            }
        }
    }

    fn record(
        &self,
        failure: LifecycleTaskFailure,
        worker: &str,
        caller: &Location<'_>,
    ) -> RecordedFailure {
        let phase = failure.observation.phase.as_str();
        let origin = failure.observation.origin.as_str();
        let cause = failure.observation.cause_class.as_str();
        let attempt_elapsed_ms = failure.observation.elapsed_ms;
        let error = failure.into_compatibility();
        let unavailable = matches!(error, SandboxStoreError::Unavailable(_));
        if unavailable {
            self.unavailable_retries.fetch_add(1, Ordering::SeqCst);
        }
        if error == SandboxStoreError::Conflict {
            self.conflicts.fetch_add(1, Ordering::SeqCst);
        }
        let unavailable_count = self.unavailable_retries.load(Ordering::SeqCst);
        let conflict_count = self.conflicts.load(Ordering::SeqCst);
        let diagnostic = format!(
            "task-home worker={worker} phase={phase} origin={origin} cause={cause} error={} desktop={} callsite={caller} elapsed_ms={} attempt_elapsed_ms={attempt_elapsed_ms} unavailable_count={unavailable_count} conflict_count={conflict_count}",
            error_class(&error), self.desktop, self.started.elapsed().as_millis(),
        );
        *self.last_failure.lock().unwrap() = Some(diagnostic.clone());
        eprintln!("{diagnostic}");
        RecordedFailure {
            diagnostic,
            retry: unavailable && unavailable_count <= 150,
            unavailable,
        }
    }
}

struct RecordedFailure {
    diagnostic: String,
    retry: bool,
    unavailable: bool,
}

impl RecordedFailure {
    fn finish<T>(self, state: String) -> Option<T> {
        let diagnostic = format!("{} after_attempt_state=[{state}]", self.diagnostic);
        if self.retry {
            eprintln!("{diagnostic}");
            return None;
        }
        panic!(
            "{diagnostic}; {}",
            if self.unavailable {
                "store unavailable retry budget exhausted"
            } else {
                "non-retryable worker failure"
            }
        );
    }
}

fn error_class(error: &SandboxStoreError) -> &'static str {
    use SandboxStoreError::*;
    match error {
        AccountDeletionInProgress => "AccountDeletionInProgress",
        Budget(_) => "Budget",
        AdmissionPaused => "AdmissionPaused",
        SessionCapacityUnavailable => "SessionCapacityUnavailable",
        NotFound => "NotFound",
        Duplicate => "Duplicate",
        Conflict => "Conflict",
        CheckpointInProgress => "CheckpointInProgress",
        LifecycleBusy { .. } => "LifecycleBusy",
        SequenceConflict => "SequenceConflict",
        AgentLogCapacityExceeded => "AgentLogCapacityExceeded",
        LimitReached { .. } => "LimitReached",
        GlobalCapacityReached => "GlobalCapacityReached",
        SandboxNotReady => "SandboxNotReady",
        WorkspaceNameConflict => "WorkspaceNameConflict",
        WorkspaceLimitReached { .. } => "WorkspaceLimitReached",
        WorkspaceHasLiveAgents => "WorkspaceHasLiveAgents",
        WorkspaceOriginConflict => "WorkspaceOriginConflict",
        WorkspaceRecoveryConflict => "WorkspaceRecoveryConflict",
        SandboxOriginConflict => "SandboxOriginConflict",
        WorkspaceNotReady => "WorkspaceNotReady",
        AgentLimitReached { .. } => "AgentLimitReached",
        AgentCreationConflict => "AgentCreationConflict",
        InvalidPage => "InvalidPage",
        InvalidTransaction(_) => "InvalidTransaction",
        Unavailable(_) => "Unavailable",
        Corrupt(_) => "Corrupt",
    }
}
