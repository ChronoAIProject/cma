use layer2_core::sandbox::store::{ReopenRefusalFence, SandboxStoreError};
use layer2_core::sandbox::Sandbox;
use mongodb::{bson::doc, ClientSession};

use super::{ApplyError, MongoSandboxStore};
use crate::store::{agent_conversation_store::checkpoint, agent_ownership::FenceError, agents};

pub(super) async fn fence(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    refusal: &ReopenRefusalFence,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let stale = || ApplyError::Domain(SandboxStoreError::Conflict);
    if let Some(lanes) = &store.released_lanes {
        let released = refusal.released.as_ref().ok_or_else(stale)?;
        let lane = sandbox.managed_runtime_lane().ok_or_else(stale)?;
        let active = lane.version_in(released).ok_or_else(stale)?;
        if sandbox.admitted_runtime_update_target() != Some(active) {
            return Err(stale());
        }
        // The terminal refusal is valid only while the captured image is active.
        lanes
            .fence_snapshot(released, lane, session)
            .await
            .map_err(ApplyError::Domain)?;
    }
    match (&store.agent_ownership, &refusal.owner) {
        (Some(ownership), Some(owner))
            if owner.agent_id == refusal.agent_id && owner.epoch != 0 =>
        {
            ownership
                .fence_transaction_typed(owner, session)
                .await
                .map_err(|error| match error {
                    FenceError::Domain(error) => ApplyError::Domain(error),
                    FenceError::Driver(error) => ApplyError::Driver(error),
                })?;
        }
        (None, None) => {}
        _ => return Err(stale()),
    }

    // Every replacement-generation checkpoint commit writes the same Sandbox
    // row that parking advances, so a concurrent commit invalidates this read.
    let row = store
        .agents
        .find_one(doc! { "_id": refusal.agent_id.as_str() })
        .session(session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or_else(stale)?;
    let agent = agents::read_agent(&row).map_err(ApplyError::Domain)?;
    if agent.sandbox_id != sandbox.id
        || agent.revision != refusal.agent_revision
        || checkpoint::read(&row).map_err(ApplyError::Domain)?.as_ref()
            != refusal.checkpoint.as_deref()
    {
        return Err(stale());
    }
    Ok(())
}
