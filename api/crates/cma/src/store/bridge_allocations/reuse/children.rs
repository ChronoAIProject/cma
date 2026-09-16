use super::*;
use crate::sandbox::{
    channel_children,
    channel_provisioning::state::ChannelProvisioningPhase,
    channel_reuse::{children as model, requests},
};
use crate::store::{sandbox_mongo::record_bson, workspaces::workspace_document};
use layer2_core::sandbox::{SandboxEventKind, WorkspaceStatus};
use mongodb::bson::Bson;

pub(super) async fn apply(
    store: &MongoBridgeAllocationStore,
    authority: &AllocationAuthority,
    current: &ChannelReuseSnapshot,
    previous: &ChannelReuseSnapshot,
    evidence: &ChannelChildEvidence,
    session: &mut ClientSession,
) -> Result<ChannelReuseSnapshot, MutationError> {
    if current != previous {
        return Err(AllocationError::Conflict.into());
    }
    current.require(authority)?;
    store
        .require_reuse_continuation_capacity(&current.allocation, session)
        .await?;
    let database = store.bridges.client.database(&store.records.namespace().db);
    let resources = &store
        .invokes
        .as_ref()
        .ok_or(AllocationError::AuthorityUnavailable)?
        .resources;
    let workspaces: Collection<Document> = majority_collection(&database, "workspaces");
    let mut next = current.clone();
    let (mut workspace, creation) = if let Some(workspace) = &current.workspace {
        (workspace.clone(), current.workspace_creation.clone())
    } else if matches!(evidence, ChannelChildEvidence::WorkspaceAdmission) {
        let (workspace, creation) = model::workspace(current)?;
        (workspace, Some(creation))
    } else {
        return Err(AllocationError::Corrupt.into());
    };
    let revision = workspace.revision;
    let stamp = layer2_core::rfc3339_at(std::time::SystemTime::now());
    let (phase, receipt) = match (current.state.phase, evidence) {
        (
            ChannelProvisioningPhase::WorkspaceAdmission,
            ChannelChildEvidence::WorkspaceAdmission,
        ) if current.workspace.is_none() => {
            if workspaces.find_one(doc! {
                "owned_workspace_cleanup_admission_v1":{"$exists":true},
                "$and":[{"$or":[{"sandbox_id":workspace.sandbox_id.as_str()},{"record.sandbox_id":workspace.sandbox_id.as_str()}]},
                    {"$or":[{"name":&workspace.name},{"record.name":&workspace.name},{"path":&workspace.path},{"record.path":&workspace.path}]}]
            }).session(&mut *session).await?.is_some() { return Err(AllocationError::IncompatibleTarget.into()); }
            if workspaces
                .count_documents(
                    doc! {"sandbox_id":workspace.sandbox_id.as_str(),"removed_at":Bson::Null,
                    "$or":[{"name":&workspace.name},{"path":&workspace.path}]},
                )
                .session(&mut *session)
                .await?
                != 0
            {
                return Err(AllocationError::IncompatibleTarget.into());
            }
            let mut document =
                workspace_document(&workspace).map_err(|_| AllocationError::Corrupt)?;
            let creation = creation.as_ref().ok_or(AllocationError::Corrupt)?;
            document.insert(
                "channel_reuse_creation",
                record_bson(creation).map_err(|_| AllocationError::Corrupt)?,
            );
            workspaces
                .insert_one(document)
                .session(&mut *session)
                .await?;
            transaction::event(
                resources,
                current,
                SandboxEventKind::WorkspaceCreated {
                    workspace_id: workspace.id.clone(),
                },
                session,
            )
            .await?;
            (
                ChannelProvisioningPhase::WorkspacePrepare,
                record_bson(creation).map_err(|_| AllocationError::Corrupt)?,
            )
        }
        (
            ChannelProvisioningPhase::WorkspacePrepare,
            ChannelChildEvidence::WorkspacePrepare { request, receipt },
        ) => {
            if creation.is_none()
                || workspace.status != WorkspaceStatus::Preparing
                || workspace.resolved_commit.is_some()
                || request != &requests::prepare(current, &workspace)
                || !channel_children::valid_prepare(request, receipt, &workspace)
            {
                return Err(AllocationError::IdentityMismatch.into());
            }
            workspace
                .record_resolved_commit(receipt.resolved_commit.clone())
                .map_err(|_| AllocationError::Corrupt)?;
            workspace.revision = workspace
                .revision
                .checked_add(1)
                .ok_or(AllocationError::Corrupt)?;
            (
                ChannelProvisioningPhase::WorkspaceSetup,
                record_bson(receipt).map_err(|_| AllocationError::Corrupt)?,
            )
        }
        (
            ChannelProvisioningPhase::WorkspaceSetup,
            ChannelChildEvidence::WorkspaceSetup { request, receipt },
        ) => {
            if creation.is_none()
                || workspace.status != WorkspaceStatus::Preparing
                || workspace.resolved_commit.is_none()
                || request != &requests::setup(current, &workspace)
                || !channel_children::valid_setup(request, receipt)
            {
                return Err(AllocationError::IdentityMismatch.into());
            }
            workspace
                .transition(WorkspaceStatus::Ready, stamp, None)
                .map_err(|_| AllocationError::Corrupt)?;
            transaction::event(
                resources,
                current,
                SandboxEventKind::WorkspaceReady {
                    workspace_id: workspace.id.clone(),
                    resolved_commit: workspace
                        .resolved_commit
                        .clone()
                        .ok_or(AllocationError::Corrupt)?,
                },
                session,
            )
            .await?;
            (
                ChannelProvisioningPhase::AgentAdmission,
                record_bson(receipt).map_err(|_| AllocationError::Corrupt)?,
            )
        }
        (
            ChannelProvisioningPhase::AgentAdmission,
            ChannelChildEvidence::AgentAdmission { capabilities },
        ) => {
            if current.agent.is_some() || current.worktree.is_some() {
                return Err(AllocationError::Conflict.into());
            }
            let (worktree, agent, provenance, log) = model::agent(
                authority,
                &current.allocation,
                &current.sandbox,
                &workspace,
                capabilities.clone(),
            )?;
            let worktrees: Collection<Document> = majority_collection(&database, "worktrees");
            let agents: Collection<Document> = majority_collection(&database, "agents");
            // Reuse the exact shared Workspace counter, then CAS its revision.
            // Existing reservations and unrelated live paths/branches all veto collision.
            if worktrees
                .count_documents(
                    doc! {"workspace_id":workspace.id.as_str(),"$or":[{"name":&worktree.name},
                    {"path":&worktree.path},{"branch":&worktree.branch}]},
                )
                .session(&mut *session)
                .await?
                != 0
                || agents
                    .count_documents(doc! {"_id":agent.id.as_str()})
                    .session(&mut *session)
                    .await?
                    != 0
            {
                return Err(AllocationError::IncompatibleTarget.into());
            }
            workspace.next_worktree_number = workspace
                .next_worktree_number
                .checked_add(1)
                .ok_or(AllocationError::Corrupt)?;
            workspace.revision = workspace
                .revision
                .checked_add(1)
                .ok_or(AllocationError::Corrupt)?;
            worktrees
                .insert_one(
                    crate::store::worktrees::worktree_document(&worktree)
                        .map_err(|_| AllocationError::Corrupt)?,
                )
                .session(&mut *session)
                .await?;
            agents
                .insert_one(
                    crate::store::agents::agent_document(&agent)
                        .map_err(|_| AllocationError::Corrupt)?,
                )
                .session(&mut *session)
                .await?;
            let rows: Collection<Document> =
                majority_collection(&database, "agent_creation_provenance");
            rows.insert_one(
                crate::store::agent_provenance::document(&provenance)
                    .map_err(|_| AllocationError::Corrupt)?,
            )
            .session(&mut *session)
            .await?;
            let rows: Collection<Document> = majority_collection(&database, "agent_log");
            rows.insert_one(
                crate::store::agent_log::log_document(&log)
                    .map_err(|_| AllocationError::Corrupt)?,
            )
            .session(&mut *session)
            .await?;
            transaction::event(
                resources,
                current,
                SandboxEventKind::WorktreeCreated {
                    workspace_id: workspace.id.clone(),
                    worktree_id: worktree.id.clone(),
                },
                session,
            )
            .await?;
            transaction::event(
                resources,
                current,
                SandboxEventKind::AgentCreated {
                    workspace_id: workspace.id.clone(),
                    worktree_id: worktree.id.clone(),
                    agent_id: agent.id.clone(),
                },
                session,
            )
            .await?;
            let receipt = record_bson(&provenance).map_err(|_| AllocationError::Corrupt)?;
            next.worktree = Some(worktree);
            next.agent = Some(agent);
            next.creation = Some(provenance);
            (ChannelProvisioningPhase::AgentStart, receipt)
        }
        _ => return Err(AllocationError::InvalidTransition.into()),
    };
    if current.workspace.is_some() {
        transaction::replace_fields(
            &database,
            "workspaces",
            workspace.id.as_str(),
            revision,
            workspace_document(&workspace).map_err(|_| AllocationError::Corrupt)?,
            session,
        )
        .await?;
    }
    next.state = current.state.advance(phase, &receipt)?;
    next.workspace = Some(workspace);
    next.workspace_creation = creation;
    transaction::save_state(store, current, &next.state, session).await?;
    // A completed child stage has no outstanding runtime effect. Release its
    // exact claim in the same transaction so the next scheduler tick can resume.
    let claim = current
        .allocation
        .lease
        .as_ref()
        .ok_or(AllocationError::LeaseUnavailable)?;
    let release = layer2_core::bridge::allocation::AllocationMutation::progress(
        current.allocation.clone(),
        authority.clone(),
        claim,
        layer2_core::bridge::allocation::AllocationProgress::NeedsConnection,
        mongodb::bson::DateTime::now().timestamp_millis(),
    )?;
    next.allocation = release.record().clone();
    store
        .replace(&current.allocation, &next.allocation, session)
        .await?;
    next.require(authority)?;
    Ok(next)
}
