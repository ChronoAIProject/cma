//! Startup reconciliation outlives a briefly unreachable runtime or store.
//!
//! The pass runs before workers or HTTP serving start. Ending the process on a
//! transient `Unavailable` only made Kubernetes restart the pod to repeat the
//! same pass from scratch, and every such restart fails a release that is
//! verifying the new pods. Retrying in process repeats it with fresh reads.
//! Bindings an earlier attempt left unresolved keep their process-local holds,
//! so a retry defers them, and every startup cleanup phase, to the hold-aware
//! background sweep. Holds only add protection; they never grant authority.
use layer2_core::sandbox::SandboxStoreError;
use std::{future::Future, time::Duration};

/// Waits after the first and second unavailable attempts; the third is final.
pub(super) const DELAYS: [Duration; 2] = [Duration::from_secs(5), Duration::from_secs(15)];

pub(super) async fn retry_unavailable<F, Fut>(
    delays: &[Duration],
    mut attempt: F,
) -> Result<(), SandboxStoreError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(), SandboxStoreError>>,
{
    let mut number = 1;
    loop {
        let error = match attempt().await {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        // Every other error keeps its fatal startup meaning.
        let (SandboxStoreError::Unavailable(_), Some(delay)) = (&error, delays.get(number - 1))
        else {
            return Err(error);
        };
        tracing::warn!(target: "cma_startup", parent: None,
            attempt = number, attempt_limit = delays.len() + 1,
            retry_in_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
            error = %error,
            "Sandbox startup reconciliation unavailable; retrying before serving");
        tokio::time::sleep(*delay).await;
        number += 1;
    }
}

#[cfg(test)]
#[path = "startup_retry_tests.rs"]
mod tests;
