//! Choose the durable transition before the worker can schedule another effect.
use crate::store::native_prepared::NativePreparedWorkspaceProof;
use layer2_core::sandbox::{WarmSandbox, WarmSandboxState};
use layer3_adapter::{AdapterError, Result};

pub(super) type PreparedStep = (WarmSandbox, Option<NativePreparedWorkspaceProof>);

pub(in crate::sandbox) fn complete_step(
    original: &WarmSandbox,
    original_proof: Option<&NativePreparedWorkspaceProof>,
    result: Result<PreparedStep>,
) -> (WarmSandbox, Option<NativePreparedWorkspaceProof>, u64) {
    match result {
        Ok((next, proof)) => {
            let delay = if next.state == WarmSandboxState::Ready {
                5
            } else {
                0
            };
            (next, proof, delay)
        }
        Err(error) => {
            // Readiness is not retirement authority during initial preparation.
            // Preserve the separate Ready maintenance policy for unhealthy spares.
            let retry = matches!(
                error,
                AdapterError::Unavailable(_) | AdapterError::NativeClaimNotReady(_)
            ) && matches!(
                original.state,
                WarmSandboxState::Preparing | WarmSandboxState::Installing
            );
            // Runtime error details can contain output. Log only safe identity and disposition.
            tracing::warn!(sandbox_id = %original.id, retry, "native preparation effect deferred");
            let mut next = original.clone();
            if retry {
                (next, original_proof.cloned(), 5)
            } else {
                next.state = WarmSandboxState::Retiring;
                (next, None, 30)
            }
        }
    }
}
