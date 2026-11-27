//! Back off only between definitively aborted Agent publication transactions.
use super::*;

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
    let code = last.and_then(driver_code);
    tracing::warn!(
        agent_id = %mutation.agent.id,
        authority_fence = mutation.fence,
        driver_code = code,
        "Agent publication contention exhausted the bounded retry budget"
    );
    // This exit is reachable only before commit or after Mongo explicitly
    // aborted the transaction. Unknown commits retain their reconciliation
    // path and must never acquire this retry/release classification.
    SandboxStoreError::Conflict
}

fn driver_code(error: &mongodb::error::Error) -> Option<i32> {
    match error.kind.as_ref() {
        mongodb::error::ErrorKind::Command(error) => Some(error.code),
        mongodb::error::ErrorKind::Write(mongodb::error::WriteFailure::WriteError(error)) => {
            Some(error.code)
        }
        _ => None,
    }
}

pub(super) fn phase(
    mutation: &AgentWorkMutation,
    phase: &'static str,
    error: ApplyError,
) -> ApplyError {
    if matches!(&error, ApplyError::Driver(driver) if driver_code(driver) == Some(112)) {
        tracing::warn!(agent_id = %mutation.agent.id, authority_fence = mutation.fence, phase,
            work_step = ?mutation.agent.work.step, driver_code = 112,
            "Agent work publication write conflict");
    }
    error
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
