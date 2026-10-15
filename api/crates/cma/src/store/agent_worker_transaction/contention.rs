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
