use super::*;
use crate::store::{
    agent_control_lineage::{fence_restart_lineage, load_control_snapshot, LineageError},
    agent_control_transaction::{
        restart::ACTIVE_RESTART_FIELD,
        restart_dispatch::{read_work, DISPATCH_FIELD},
        restart_live_lease_expression,
    },
};
use layer2_core::sandbox::{
    store::{AgentControlTarget, AgentWorkPublicationLineage},
    SandboxStatus, WorkspaceStatus, WorktreeStatus,
};

pub(in crate::store) const CONNECTION_FIELD: &str = "control_restart_connection_v1";

pub(super) fn fence_live_lease(filter: &mut Document, current: &Agent) {
    if current.work.step == AgentWorkStep::RestartConversation {
        filter.insert("$expr", restart_live_lease_expression());
    }
}

pub(super) async fn finish(
    store: &MongoSandboxStore,
    (row, current, next): (&Document, &Agent, &Agent),
    parents: Option<&AgentWorkPublicationParents>,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    if current.work.step != AgentWorkStep::RestartConversation || next.status != AgentStatus::Ready
    {
        return Ok(());
    }

    let parents = parents.ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?;
    let restart = read_work(row, current).map_err(ApplyError::Domain)?;
    let open = restart
        .dispatched_open
        .ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?;
    // The ordinary writer has already staged the Agent CAS in this transaction.
    let snapshot = load_control_snapshot(store, &AgentControlTarget::from(next), session, false)
        .await
        .map_err(lineage_error)?;
    if snapshot.agent != *next
        || parents.lineage != AgentWorkPublicationLineage::Authorized
        || snapshot.sandbox.revision != parents.sandbox_revision
        || snapshot.workspace.revision != parents.workspace_revision
        || snapshot.worktree.revision != parents.worktree_revision
        || snapshot.worktree.revision != restart.acceptance.source.worktree_revision
        || snapshot.sandbox.status != SandboxStatus::Active
        || snapshot.sandbox.lifecycle.operation.is_some()
        || snapshot.sandbox.lifecycle.checkpoint_capture.is_some()
        || !snapshot.sandbox.credential.delivered
        || snapshot
            .sandbox
            .lifecycle
            .home
            .as_ref()
            .and_then(|home| home.attachment())
            .and_then(|home| home.identity().ok())
            .as_ref()
            != Some(&restart.acceptance.source.runtime.runtime.home_identity)
        || snapshot.workspace.status != WorkspaceStatus::Ready
        || snapshot.worktree.status != WorktreeStatus::Ready
        || snapshot.sandbox.runtime.as_ref().is_none_or(|binding| {
            binding.boot_id.as_ref() != Some(&restart.acceptance.source.runtime.boot_id)
                || binding.provision_generation
                    != Some(restart.acceptance.source.runtime.runtime.generation)
        })
    {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    fence_restart_lineage(
        store,
        &snapshot,
        &format!("{}:ready", restart.acceptance.operation_id),
        session,
    )
    .await
    .map_err(lineage_error)?;
    let settled = store
        .agents
        .update_one(
            doc! {
                "_id": next.id.as_str(),
                "revision": signed(next.revision, "restart publication revision")
                    .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
                "control_restart_active_v1.operation_id": &restart.acceptance.operation_id,
                "control_restart_dispatch_v1.operation_id": &open.operation_id,
                "control_restart_dispatch_v1.request_sha256": &open.request_sha256,
            },
            doc! {
                "$set": {(CONNECTION_FIELD): record_bson(&open)
                    .map_err(SandboxStoreError::Corrupt).map_err(ApplyError::Domain)?},
                "$unset": {(ACTIVE_RESTART_FIELD): "", (DISPATCH_FIELD): ""},
            },
        )
        .session(session)
        .await
        .map_err(ApplyError::Driver)?;
    if settled.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    Ok(())
}

fn lineage_error(error: LineageError) -> ApplyError {
    match error {
        LineageError::Driver(error) => ApplyError::Driver(error),
        LineageError::ReceiptCorrupt(detail) => {
            ApplyError::Domain(SandboxStoreError::Corrupt(detail))
        }
        LineageError::Domain(_) => ApplyError::Domain(SandboxStoreError::Conflict),
    }
}
