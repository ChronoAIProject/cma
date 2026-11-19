//! Only definitively aborted transactions may restart from a fresh observation.

use std::time::Duration;

use layer2_core::sandbox::store::SandboxStoreError;

/// Jitter de-synchronizes writers that collided, adding at most 15 ms per retry.
/// The attempt count and error semantics are unchanged.
pub(in crate::store) fn delay(attempt: usize, jitter: u64) -> Duration {
    if attempt == 0 {
        return Duration::ZERO;
    }
    Duration::from_millis(
        (10_u64 << (attempt - 1).min(3)) + (jitter.rotate_left(attempt as u32) & 15),
    )
}

/// Draw fresh jitter for each retry; the first attempt neither draws nor sleeps.
pub(in crate::store) async fn backoff(attempt: usize) {
    if attempt > 0 {
        // Give the competing writer time to commit instead of exhausting all
        // five attempts in one burst. Each retry opens a new snapshot/session.
        let jitter = uuid::Uuid::new_v4().as_u128() as u64;
        tokio::time::sleep(delay(attempt, jitter)).await;
    }
}

pub(in crate::store) fn exhausted(last_transient: Option<String>) -> SandboxStoreError {
    // Reaching this branch means every attempt failed before any possible commit
    // or Mongo explicitly labelled the transaction aborted. Unknown commit
    // results return through their separate reconciliation path, never here.
    tracing::warn!(
        reason = last_transient.as_deref().unwrap_or("no driver result"),
        "Conversation transaction contention exhausted the bounded retry budget"
    );
    SandboxStoreError::Conflict
}

#[cfg(test)]
#[path = "contention_tests.rs"]
mod tests;
