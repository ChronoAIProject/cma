//! Keep approved access pending while an owner has stopped the runtime.
use std::future::Future;

use layer2_core::sandbox::Sandbox;

use super::*;

pub(super) async fn schedule(
    store: &MongoSandboxStore,
    record: &AgentAccessRecord,
    sandbox: &Sandbox,
    connections_ready: impl Future<Output = Result<bool, AgentAccessError>>,
) -> Result<(), AgentAccessError> {
    let lease = record.lease.as_ref().ok_or(AgentAccessError::Conflict)?;
    // Approval remains valid while paused or in transition, but is not consent
    // to undo an owner's pause. Their current Active runtime receives the new
    // credential in place; an active Agent turn is not a reason to stop it.
    if sandbox.status == SandboxStatus::Active && connections_ready.await? {
        store.grant_live_access_request_services(lease).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
