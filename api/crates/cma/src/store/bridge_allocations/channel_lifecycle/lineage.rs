//! Revival preserves the actual originally admitted child tree and Agent.
use super::*;
use layer2_core::{
    bridge::allocation::AllocationPlacement,
    sandbox::{AgentCreationOrigin, WorkspaceStatus, WorktreeStatus},
};
use mongodb::Database;

pub(super) async fn require(
    database: &Database,
    snapshot: &ChannelLifecycleSnapshot,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let target = snapshot.allocation.placement.target();
    let workspace_row = read(
        database,
        "workspaces",
        target.workspace_id.as_str(),
        session,
    )
    .await?;
    let tree_row = read(database, "worktrees", target.worktree_id.as_str(), session).await?;
    let agent_row = read(database, "agents", target.agent_id.as_str(), session).await?;
    let origin_row = read(
        database,
        "agent_creation_provenance",
        target.agent_id.as_str(),
        session,
    )
    .await?;
    let workspace = crate::store::workspaces::read_workspace(&workspace_row)
        .map_err(|_| AllocationError::Corrupt)?;
    let tree =
        crate::store::worktrees::read_worktree(&tree_row).map_err(|_| AllocationError::Corrupt)?;
    let agent =
        crate::store::agents::read_agent(&agent_row).map_err(|_| AllocationError::Corrupt)?;
    let origin =
        crate::store::agent_provenance::read(&origin_row).map_err(|_| AllocationError::Corrupt)?;
    origin
        .validate_against(&agent, &snapshot.sandbox)
        .map_err(|_| AllocationError::IdentityMismatch)?;
    let AllocationPlacement::NewSandbox { profiles, .. } = &snapshot.allocation.placement else {
        return Err(AllocationError::IdentityMismatch.into());
    };
    let expected_origin = AgentCreationOrigin::ChannelBridge {
        bridge_id: snapshot.allocation.identity.bridge_id.as_str().into(),
        partition_id: snapshot.allocation.identity.key.hash().into(),
        generation: snapshot.allocation.identity.generation,
        source_reference: snapshot.allocation.first_source.inbox_id().into(),
    };
    if workspace.id != target.workspace_id
        || workspace.sandbox_id != snapshot.sandbox.id
        || workspace.owner != snapshot.sandbox.owner
        || workspace.status != WorkspaceStatus::Ready
        || tree.id != target.worktree_id
        || tree.workspace_id != workspace.id
        || tree.sandbox_id != snapshot.sandbox.id
        || tree.owner != snapshot.sandbox.owner
        || tree.status != WorktreeStatus::Ready
        || tree.agent_id.as_ref() != Some(&agent.id)
        || agent.id != target.agent_id
        || agent.workspace_id != workspace.id
        || agent.worktree_id != tree.id
        || agent.sandbox_id != snapshot.sandbox.id
        || agent.owner != snapshot.sandbox.owner
        || !agent.status.is_live()
        || agent.requested_stop_reason.is_some()
        || origin.creation_origin != expected_origin
        || origin.owner != snapshot.allocation.owner
        || origin.created_by.as_ref() != Some(&profiles.approved_by)
        || agent.creation_key != format!("channel-allocation:{}", snapshot.allocation.id)
        || agent.creation_fingerprint
            != layer2_core::sandbox::channel_fingerprint(&snapshot.allocation)
                .map_err(|_| AllocationError::Corrupt)?
        || agent.profile != profiles.agent
        || workspace.resolved_commit.as_ref() != Some(&tree.base_commit)
        || agent.conversation.conversation_id.is_none()
    {
        return Err(AllocationError::IdentityMismatch.into());
    }
    for (name, row) in [
        ("workspaces", workspace_row),
        ("worktrees", tree_row),
        ("agents", agent_row),
        ("agent_creation_provenance", origin_row),
    ] {
        let rows: Collection<Document> = majority_collection(database, name);
        let changed = rows
            .update_one(row, doc! {"$inc":{"channel_lifecycle_fence":1_i64}})
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(AllocationError::Conflict.into());
        }
    }
    Ok(())
}
async fn read(
    database: &Database,
    name: &str,
    id: &str,
    session: &mut ClientSession,
) -> Result<Document, MutationError> {
    let rows: Collection<Document> = majority_collection(database, name);
    rows.find_one(doc! {"_id":id})
        .session(session)
        .await?
        .ok_or(AllocationError::Corrupt.into())
}
