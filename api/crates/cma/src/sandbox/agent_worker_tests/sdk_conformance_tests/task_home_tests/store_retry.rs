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
}

impl StoreRetry {
    pub(super) fn new(desktop: bool) -> Self {
        Self {
            desktop,
            started: tokio::time::Instant::now(),
            unavailable_retries: AtomicUsize::new(0),
            conflicts: AtomicUsize::new(0),
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
            let state = if result.is_err() {
                snapshot::agent(harness, id).await
            } else {
                String::new()
            };
            self.check(result, "Agent", caller, &state)
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
            let state = if result.is_err() {
                snapshot::sandbox(harness, id).await
            } else {
                String::new()
            };
            self.check(result, "Sandbox", caller, &state)
        }
    }

    fn check<T>(
        &self,
        result: Result<T, LifecycleTaskFailure>,
        worker: &str,
        caller: &Location<'_>,
        state: &str,
    ) -> Option<T> {
        let failure = match result {
            Ok(value) => return Some(value),
            Err(failure) => failure,
        };
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
            "task-home worker={worker} phase={phase} origin={origin} cause={cause} error={} desktop={} callsite={caller} elapsed_ms={} attempt_elapsed_ms={attempt_elapsed_ms} unavailable_count={unavailable_count} conflict_count={conflict_count} after_attempt_state=[{state}]",
            error_class(&error), self.desktop, self.started.elapsed().as_millis(),
        );
        if unavailable && unavailable_count <= 150 {
            eprintln!("{diagnostic}");
            return None;
        }
        panic!(
            "{diagnostic}; {}",
            if unavailable {
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
