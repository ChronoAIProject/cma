//! Back off only between definitively aborted Agent publication transactions.
use super::*;
use sha2::{Digest, Sha256};

pub(super) enum PublicationFailure {
    KnownAbortedCas,
    OwnerAuthorityRefused,
    WorkLeaseChanged,
    UnknownCommit,
}

pub(super) fn diagnose(mutation: &AgentWorkMutation, failure: PublicationFailure) {
    let failure = match failure {
        PublicationFailure::KnownAbortedCas => "known_aborted_cas",
        PublicationFailure::OwnerAuthorityRefused => "owner_authority_refused",
        PublicationFailure::WorkLeaseChanged => "work_lease_changed",
        PublicationFailure::UnknownCommit => "unknown_commit",
    };
    tracing::info!(
        target: "agent_scheduler_failure",
        phase = "publication",
        failure,
        next_step = mutation.agent.work.step.operation_name(),
        agent_sha256 = %format_args!("{:x}", Sha256::digest(mutation.agent.id.as_str().as_bytes())),
        "Agent publication boundary"
    );
}

pub(super) fn diagnose_write_conflict(mutation: &AgentWorkMutation, error: &mongodb::error::Error) {
    const WRITE_CONFLICT: i32 = 112;
    if code(error) == Some(WRITE_CONFLICT) {
        diagnose(mutation, PublicationFailure::KnownAbortedCas);
    }
}

pub(super) fn diagnose_prepared_commit(mutation: &AgentWorkMutation) {
    let work_lease = format!("{}:{}", mutation.fence, mutation.holder);
    tracing::info!(
        target: "agent_scheduler_failure",
        phase = "prepared_commit",
        agent_sha256 = %format_args!("{:x}", Sha256::digest(mutation.agent.id.as_str().as_bytes())),
        work_lease_sha256 = %format_args!("{:x}", Sha256::digest(work_lease.as_bytes())),
        "Prepared Agent Ready publication committed"
    );
}

pub(super) fn ownership_error(
    mutation: &AgentWorkMutation,
    error: super::super::agent_ownership::FenceError,
) -> ApplyError {
    use super::super::agent_ownership::FenceError;
    match error {
        FenceError::Domain(error) => {
            if matches!(&error, SandboxStoreError::Conflict) {
                diagnose(mutation, PublicationFailure::OwnerAuthorityRefused);
            }
            ApplyError::Domain(error)
        }
        FenceError::Driver(error) => ApplyError::Driver(error),
    }
}

pub(super) fn validation_error(
    current: &Agent,
    mutation: &AgentWorkMutation,
    error: SandboxStoreError,
) -> ApplyError {
    if matches!(&error, SandboxStoreError::Conflict)
        && !current.work.owns(&mutation.holder, mutation.fence)
    {
        diagnose(mutation, PublicationFailure::WorkLeaseChanged);
    }
    ApplyError::Domain(error)
}

/// Reuse the transaction's seed to de-synchronize writers that collided.
/// Each retry adds at most 15 ms; attempt count and error semantics are unchanged.
pub(super) async fn backoff(attempt: usize, jitter: u64) {
    if attempt > 0 {
        tokio::time::sleep(crate::store::contention::delay(attempt, jitter)).await;
    }
}

pub(super) fn exhausted(
    mutation: &AgentWorkMutation,
    last: Option<&mongodb::error::Error>,
) -> SandboxStoreError {
    tracing::warn!(
        agent_id = %mutation.agent.id,
        authority_fence = mutation.fence,
        driver_code = last.and_then(code),
        "Agent publication contention exhausted the bounded retry budget"
    );
    // This exit is reachable only before commit or after Mongo explicitly
    // aborted the transaction. Unknown commits retain their reconciliation
    // path and must never acquire this retry/release classification.
    SandboxStoreError::Conflict
}

fn code(error: &mongodb::error::Error) -> Option<i32> {
    match error.kind.as_ref() {
        mongodb::error::ErrorKind::Command(error) => Some(error.code),
        mongodb::error::ErrorKind::Write(mongodb::error::WriteFailure::WriteError(error)) => {
            Some(error.code)
        }
        _ => None,
    }
}

/// Diagnose slow publications without logging documents or driver error bodies.
pub(super) struct PublicationTiming<'a> {
    mutation: &'a AgentWorkMutation,
    started: std::time::Instant,
    queue_ms: u64,
    attempts: usize,
    committed: bool,
}

impl<'a> PublicationTiming<'a> {
    pub(super) fn new(mutation: &'a AgentWorkMutation) -> Self {
        Self {
            mutation,
            started: std::time::Instant::now(),
            queue_ms: 0,
            attempts: 0,
            committed: false,
        }
    }
    pub(super) fn acquired(&mut self) {
        self.queue_ms = self.started.elapsed().as_millis() as u64;
    }
    pub(super) fn attempt(&mut self) {
        self.attempts += 1;
    }
    pub(super) fn committed(&mut self) {
        self.committed = true;
    }
}

impl Drop for PublicationTiming<'_> {
    fn drop(&mut self) {
        let elapsed_ms = self.started.elapsed().as_millis() as u64;
        if elapsed_ms >= 100 || self.attempts > 1 {
            tracing::info!(
                agent_id = %self.mutation.agent.id,
                step = ?self.mutation.agent.work.step,
                authority_fence = self.mutation.fence,
                queue_ms = self.queue_ms,
                transaction_attempts = self.attempts,
                elapsed_ms,
                commit_confirmed = self.committed,
                "Agent work publication timing"
            );
        }
    }
}
