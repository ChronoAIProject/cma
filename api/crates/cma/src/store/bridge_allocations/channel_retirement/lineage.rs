//! Enumerate the complete root tree, including retained and removed children.
//! A later child or inconsistent query mirror prevents whole-parent cleanup.
use super::*;
use crate::sandbox::{
    channel_children::model::ChannelWorkspaceCreation, channel_retirement::model::OwnedTree,
};
use layer2_core::{
    bridge::allocation::AllocationPlacement,
    sandbox::{
        AgentCreationOrigin, Sandbox, WorkspaceOrigin, WorkspaceStatus, WorkspaceWorkStep,
        WorktreeStatus,
    },
};
use mongodb::Database;

pub(super) async fn require(
    database: &Database,
    allocation: &AllocationRecord,
    sandbox: &Sandbox,
    session: &mut ClientSession,
) -> Result<OwnedTree, MutationError> {
    let AllocationPlacement::NewSandbox { target, profiles } = &allocation.placement else {
        return Err(AllocationError::IdentityMismatch.into());
    };
    let base = vec![
        doc! {"sandbox_id":sandbox.id.as_str()},
        doc! {"record.sandbox_id":sandbox.id.as_str()},
    ];
    let workspace_row = one(
        database,
        "workspaces",
        target.workspace_id.as_str(),
        base.clone(),
        session,
    )
    .await?;
    let mut children = base;
    children.extend([
        doc! {"workspace_id":target.workspace_id.as_str()},
        doc! {"record.workspace_id":target.workspace_id.as_str()},
    ]);
    let tree_row = one(
        database,
        "worktrees",
        target.worktree_id.as_str(),
        children.clone(),
        session,
    )
    .await?;
    children.extend([
        doc! {"worktree_id":target.worktree_id.as_str()},
        doc! {"record.worktree_id":target.worktree_id.as_str()},
    ]);
    let agent_row = one(
        database,
        "agents",
        target.agent_id.as_str(),
        children.clone(),
        session,
    )
    .await?;
    let origin_row = one(
        database,
        "agent_creation_provenance",
        target.agent_id.as_str(),
        children,
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
    let creation: ChannelWorkspaceCreation = mongodb::bson::deserialize_from_bson(
        workspace_row
            .get("channel_creation")
            .cloned()
            .ok_or(AllocationError::Corrupt)?,
    )
    .map_err(|_| AllocationError::Corrupt)?;
    origin
        .validate_against(&agent, sandbox)
        .map_err(|_| AllocationError::IdentityMismatch)?;
    let expected_origin = AgentCreationOrigin::ChannelBridge {
        bridge_id: allocation.identity.bridge_id.as_str().into(),
        partition_id: allocation.identity.key.hash().into(),
        generation: allocation.identity.generation,
        source_reference: allocation.first_source.inbox_id().into(),
    };
    let name = profiles
        .workspace
        .content
        .workspace_name()
        .map_err(|_| AllocationError::InvalidProfiles)?;
    let source = match &profiles.workspace.content.location {
        layer2_core::profiles::WorkspaceLocation::GitRepository { url, reference } => {
            layer2_core::sandbox::WorkspaceSource::GitRepository {
                url: url.clone(),
                reference: reference.clone(),
            }
        }
        layer2_core::profiles::WorkspaceLocation::Directory { .. } => {
            layer2_core::sandbox::WorkspaceSource::Empty
        }
    };
    if creation.schema_version != 1
        || creation.allocation_id != allocation.id
        || creation.source != allocation.first_source
        || creation.workspace_id != workspace.id
        || creation.workspace_created_at != workspace.created_at
        || creation.sandbox_created_at != sandbox.created_at
        || creation.profile_digest != profiles.workspace.digest
        || workspace.id != target.workspace_id
        || workspace.sandbox_id != sandbox.id
        || workspace.owner != sandbox.owner
        || workspace.profile != profiles.workspace
        || workspace.origin != WorkspaceOrigin::Manual
        || workspace.name != name
        || workspace.path != format!("/home/cma/{name}")
        || workspace.source != source
        || workspace.status != WorkspaceStatus::Ready
        || workspace.next_worktree_number != 2
        || workspace.work.step != WorkspaceWorkStep::None
        || workspace.work.lease.is_some()
        || workspace.work.execution.is_some()
        || workspace.removed_at.is_some()
        || tree.id != target.worktree_id
        || tree.workspace_id != workspace.id
        || tree.sandbox_id != sandbox.id
        || tree.owner != sandbox.owner
        || tree.status != WorktreeStatus::Ready
        || tree.name != "wt-1"
        || tree.branch != "cma/wt-1"
        || tree.path != format!("/home/cma/{name}/.worktrees/wt-1")
        || tree.agent_id.as_ref() != Some(&agent.id)
        || workspace.resolved_commit.as_ref() != Some(&tree.base_commit)
        || agent.id != target.agent_id
        || agent.workspace_id != workspace.id
        || agent.worktree_id != tree.id
        || agent.sandbox_id != sandbox.id
        || agent.owner != sandbox.owner
        || agent.profile != profiles.agent
        || agent.creation_key != format!("channel-allocation:{}", allocation.id)
        || agent.creation_fingerprint
            != layer2_core::sandbox::channel_fingerprint(allocation)
                .map_err(|_| AllocationError::Corrupt)?
        || origin.creation_origin != expected_origin
        || origin.owner != allocation.owner
        || origin.created_by.as_ref() != Some(&profiles.approved_by)
    {
        return Err(AllocationError::IdentityMismatch.into());
    }
    let fingerprint = layer2_core::profiles::content_digest(&(
        "cma.channel.retirement-tree.v1",
        &creation,
        &origin,
        &workspace.path,
        &workspace.source,
        &workspace.resolved_commit,
        &workspace.ready_at,
        &tree.id,
        &tree.path,
        &tree.branch,
        &tree.base_commit,
    ))
    .map_err(|_| AllocationError::Corrupt)?;
    for (name, row) in [
        ("workspaces", workspace_row),
        ("worktrees", tree_row),
        ("agents", agent_row),
        ("agent_creation_provenance", origin_row),
    ] {
        let rows: Collection<Document> = majority_collection(database, name);
        let changed = rows
            .update_one(row, doc! {"$inc":{"channel_retirement_fence":1_i64}})
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(AllocationError::Conflict.into());
        }
    }
    Ok(OwnedTree {
        fingerprint,
        agent_live: agent.status.is_live(),
    })
}
async fn one(
    database: &Database,
    name: &str,
    id: &str,
    mut alternatives: Vec<Document>,
    session: &mut ClientSession,
) -> Result<Document, MutationError> {
    alternatives.push(doc! {"_id":id});
    let rows: Collection<Document> = majority_collection(database, name);
    let mut cursor = rows
        .find(doc! {"$or":alternatives})
        .limit(2)
        .session(&mut *session)
        .await?;
    let row = cursor
        .next(&mut *session)
        .await
        .transpose()?
        .ok_or(AllocationError::Corrupt)?;
    if cursor.next(&mut *session).await.transpose()?.is_some()
        || row.get_str("_id").ok() != Some(id)
    {
        return Err(AllocationError::IdentityMismatch.into());
    }
    Ok(row)
}
