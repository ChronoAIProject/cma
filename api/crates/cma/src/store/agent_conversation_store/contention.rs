//! Only definitively aborted transactions may restart from a fresh observation.

use std::time::Duration;

use layer2_core::sandbox::store::SandboxStoreError;

pub(super) async fn backoff(attempt: usize) {
    if attempt > 0 {
        // Give the competing writer time to commit instead of exhausting all
        // five attempts in one burst. Each retry opens a new snapshot/session.
        tokio::time::sleep(Duration::from_millis(10 << (attempt - 1).min(3))).await;
    }
}

pub(super) fn exhausted(last_transient: Option<String>) -> SandboxStoreError {
    // Reaching this branch means every attempt failed before any possible commit
    // or Mongo explicitly labelled the transaction aborted. Unknown commit
    // results return through their separate reconciliation path, never here.
    tracing::warn!(
        reason = last_transient.as_deref().unwrap_or("no driver result"),
        "Conversation transaction contention exhausted the bounded retry budget"
    );
    SandboxStoreError::Conflict
}
