//! Back off only between definitively aborted Agent publication transactions.
use super::*;

pub(super) async fn backoff(attempt: usize, jitter: u64) {
    if attempt > 0 {
        let delay = (10_u64 << (attempt - 1).min(3)) + (jitter.rotate_left(attempt as u32) & 15);
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    }
}

pub(super) fn exhausted(
    mutation: &AgentWorkMutation,
    last: Option<&mongodb::error::Error>,
) -> SandboxStoreError {
    let code = last.and_then(|error| match error.kind.as_ref() {
        mongodb::error::ErrorKind::Command(error) => Some(error.code),
        mongodb::error::ErrorKind::Write(mongodb::error::WriteFailure::WriteError(error)) => {
            Some(error.code)
        }
        _ => None,
    });
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
