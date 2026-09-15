use super::*;
use crate::sandbox::{
    channel_children::{
        self,
        model::{self, ChannelWorkspaceCreation},
    },
    channel_provisioning::state::ChannelProvisioningPhase,
};
use crate::store::{
    sandbox_mongo::record_bson,
    workspaces::{read_workspace as decode_workspace, workspace_document},
};
use layer2_core::sandbox::{SandboxEvent, SandboxEventKind, WorkspaceStatus};

pub(super) async fn apply(
    store: &MongoBridgeAllocationStore,
    authority: &AllocationAuthority,
    lease: &BridgeControlPlaneLease,
    snapshot: &ChannelProvisioningSnapshot,
    evidence: &ChannelChildEvidence,
    session: &mut ClientSession,
) -> Result<ChannelProvisioningSnapshot, MutationError> {
    snapshot.validate()?;
    authority.require_record(&snapshot.allocation)?;
    let database = store.bridges.client.database(&store.records.namespace().db);
    let workspaces: Collection<Document> = majority_collection(&database, "workspaces");
    let existing = workspaces
        .find_one(doc! {"_id":snapshot.allocation.placement.target().workspace_id.as_str()})
        .session(&mut *session)
        .await?;
    let (mut workspace, creation) = match existing.as_ref() {
        Some(document) => {
            let workspace = read_workspace(snapshot, document)?;
            let creation: ChannelWorkspaceCreation = mongodb::bson::deserialize_from_bson(
                document
                    .get("channel_creation")
                    .cloned()
                    .ok_or(AllocationError::Corrupt)?,
            )
            .map_err(|_| AllocationError::Corrupt)?;
            (workspace, creation)
        }
        None if matches!(evidence, ChannelChildEvidence::WorkspaceAdmission) => {
            model::workspace(snapshot)?
        }
        None => return Err(AllocationError::Corrupt.into()),
    };
    let previous_revision = workspace.revision;
    let stamp = layer2_core::rfc3339_at(std::time::SystemTime::now());
    let mut agent_creation = None;
    let (phase, receipt, event) = match (snapshot.state.phase, evidence) {
        (
            ChannelProvisioningPhase::WorkspaceAdmission,
            ChannelChildEvidence::WorkspaceAdmission,
        ) if existing.is_none() => {
            let live = workspaces.count_documents(doc! {"sandbox_id":snapshot.sandbox.id.as_str(),"removed_at":mongodb::bson::Bson::Null}).session(&mut *session).await?;
            if live != 0 {
                return Err(AllocationError::IdentityMismatch.into());
            }
            (
                ChannelProvisioningPhase::WorkspacePrepare,
                record_bson(&creation).map_err(|_| AllocationError::Corrupt)?,
                Some(SandboxEventKind::WorkspaceCreated {
                    workspace_id: workspace.id.clone(),
                }),
            )
        }
        (
            ChannelProvisioningPhase::WorkspacePrepare,
            ChannelChildEvidence::WorkspacePrepare { request, receipt },
        ) => {
            if workspace.status != WorkspaceStatus::Preparing
                || workspace.resolved_commit.is_some()
                || request != &channel_children::prepare_request(snapshot, &workspace)
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
                None,
            )
        }
        (
            ChannelProvisioningPhase::WorkspaceSetup,
            ChannelChildEvidence::WorkspaceSetup { request, receipt },
        ) => {
            if workspace.status != WorkspaceStatus::Preparing
                || workspace.resolved_commit.is_none()
                || request != &channel_children::setup_request(snapshot, &workspace)
                || !channel_children::valid_setup(request, receipt)
            {
                return Err(AllocationError::IdentityMismatch.into());
            }
            workspace
                .transition(WorkspaceStatus::Ready, stamp.clone(), None)
                .map_err(|_| AllocationError::Corrupt)?;
            (
                ChannelProvisioningPhase::AgentAdmission,
                record_bson(receipt).map_err(|_| AllocationError::Corrupt)?,
                Some(SandboxEventKind::WorkspaceReady {
                    workspace_id: workspace.id.clone(),
                    resolved_commit: workspace
                        .resolved_commit
                        .clone()
                        .ok_or(AllocationError::Corrupt)?,
                }),
            )
        }
        (
            ChannelProvisioningPhase::AgentAdmission,
            ChannelChildEvidence::AgentAdmission { capabilities },
        ) => {
            let creation = model::agent(
                authority,
                &snapshot.allocation,
                &snapshot.sandbox,
                &workspace,
                capabilities.clone(),
            )?;
            workspace.next_worktree_number = 2;
            workspace.revision = workspace
                .revision
                .checked_add(1)
                .ok_or(AllocationError::Corrupt)?;
            let receipt = record_bson(&creation.2).map_err(|_| AllocationError::Corrupt)?;
            agent_creation = Some(creation);
            (ChannelProvisioningPhase::AgentStart, receipt, None)
        }
        _ => return Err(AllocationError::InvalidTransition.into()),
    };
    let mut sandbox = snapshot.sandbox.clone();
    sandbox.revision = sandbox
        .revision
        .checked_add(1)
        .ok_or(AllocationError::Corrupt)?;
    sandbox.updated_at = stamp.clone();
    let state = snapshot.state.next(phase, &receipt)?;
    let change = super::super::channel_provisioning::StageChange {
        previous: snapshot,
        sandbox,
        state,
        event,
        material: None,
    };
    let progressed = super::super::channel_provisioning::transaction::apply(
        store,
        authority,
        lease,
        &snapshot.allocation,
        &snapshot.state.settings,
        Some(&change),
        session,
    )
    .await?;
    creation.require(&progressed, &workspace)?;
    let mut document = workspace_document(&workspace).map_err(|_| AllocationError::Corrupt)?;
    document.insert(
        "channel_creation",
        record_bson(&creation).map_err(|_| AllocationError::Corrupt)?,
    );
    if existing.is_none() {
        workspaces
            .insert_one(document)
            .session(&mut *session)
            .await?;
    } else {
        document.remove("_id");
        let result = workspaces
            .update_one(
                doc! {"_id":workspace.id.as_str(),"revision":previous_revision as i64},
                doc! {"$set":document},
            )
            .session(&mut *session)
            .await?;
        if result.matched_count != 1 {
            return Err(AllocationError::Conflict.into());
        }
    }
    if let Some((worktree, agent, provenance, log)) = agent_creation {
        use crate::store::{
            agent_log::log_document, agent_provenance, agents::agent_document,
            worktrees::worktree_document,
        };
        let worktrees: Collection<Document> = majority_collection(&database, "worktrees");
        let agents: Collection<Document> = majority_collection(&database, "agents");
        if worktrees
            .count_documents(doc! {"workspace_id":workspace.id.as_str()})
            .session(&mut *session)
            .await?
            != 0
            || agents
                .count_documents(doc! {"workspace_id":workspace.id.as_str()})
                .session(&mut *session)
                .await?
                != 0
        {
            return Err(AllocationError::IdentityMismatch.into());
        }
        worktrees
            .insert_one(worktree_document(&worktree).map_err(|_| AllocationError::Corrupt)?)
            .session(&mut *session)
            .await?;
        agents
            .insert_one(agent_document(&agent).map_err(|_| AllocationError::Corrupt)?)
            .session(&mut *session)
            .await?;
        let provenance_rows: Collection<Document> =
            majority_collection(&database, "agent_creation_provenance");
        provenance_rows
            .insert_one(
                agent_provenance::document(&provenance).map_err(|_| AllocationError::Corrupt)?,
            )
            .session(&mut *session)
            .await?;
        let logs: Collection<Document> = majority_collection(&database, "agent_log");
        logs.insert_one(log_document(&log).map_err(|_| AllocationError::Corrupt)?)
            .session(&mut *session)
            .await?;
        append_events(
            &database,
            &progressed,
            &[
                SandboxEventKind::WorktreeCreated {
                    workspace_id: workspace.id.clone(),
                    worktree_id: worktree.id.clone(),
                },
                SandboxEventKind::AgentCreated {
                    workspace_id: workspace.id,
                    worktree_id: worktree.id,
                    agent_id: agent.id,
                },
            ],
            session,
        )
        .await?;
    }
    Ok(progressed)
}

pub(in crate::store::bridge_allocations) fn read_workspace(
    snapshot: &ChannelProvisioningSnapshot,
    document: &Document,
) -> Result<Workspace, AllocationError> {
    let workspace = decode_workspace(document).map_err(|_| AllocationError::Corrupt)?;
    let creation: ChannelWorkspaceCreation = mongodb::bson::deserialize_from_bson(
        document
            .get("channel_creation")
            .cloned()
            .ok_or(AllocationError::Corrupt)?,
    )
    .map_err(|_| AllocationError::Corrupt)?;
    creation.require(snapshot, &workspace)?;
    Ok(workspace)
}

async fn append_events(
    database: &mongodb::Database,
    snapshot: &ChannelProvisioningSnapshot,
    kinds: &[SandboxEventKind],
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let sandboxes: Collection<Document> = majority_collection(database, "sandboxes");
    let events: Collection<Document> = majority_collection(database, "sandbox_events");
    let document = sandboxes
        .find_one(doc! {"_id":snapshot.sandbox.id.as_str()})
        .session(&mut *session)
        .await?
        .ok_or(AllocationError::Corrupt)?;
    let mut seq = document
        .get_i64("last_seq")
        .map_err(|_| AllocationError::Corrupt)?;
    for kind in kinds {
        seq = seq.checked_add(1).ok_or(AllocationError::Corrupt)?;
        events
            .insert_one(
                crate::store::sandbox_events::event_document(&SandboxEvent {
                    sandbox_id: snapshot.sandbox.id.clone(),
                    seq: seq as u64,
                    appended_at: snapshot.sandbox.updated_at.clone(),
                    kind: kind.clone(),
                })
                .map_err(|_| AllocationError::Corrupt)?,
            )
            .session(&mut *session)
            .await?;
    }
    let result = sandboxes
        .update_one(
            doc! {"_id":snapshot.sandbox.id.as_str(),"revision":snapshot.sandbox.revision as i64},
            doc! {"$set":{"last_seq":seq}},
        )
        .session(&mut *session)
        .await?;
    if result.matched_count != 1 {
        return Err(AllocationError::Conflict.into());
    }
    Ok(())
}
