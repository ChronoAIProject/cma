//! Exact allocation-owned child builders; reused roots and siblings are retained.
use super::state::ChannelReuseSnapshot;
use layer2_core::bridge::{
    allocation::{AllocationAuthority, AllocationError, AllocationPlacement, AllocationRecord},
    partition::PartitionSourcePin,
};
use layer2_core::profiles::WorkspaceLocation;
use layer2_core::sandbox::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReuseWorkspaceCreation {
    pub schema_version: u16,
    pub allocation_id: String,
    pub source: PartitionSourcePin,
    pub workspace_id: layer2_core::WorkspaceId,
    pub workspace_created_at: String,
    pub sandbox_created_at: String,
    pub profile_digest: String,
}

pub(crate) fn workspace_name(allocation: &AllocationRecord) -> Result<String, AllocationError> {
    let digest = layer2_core::profiles::content_digest(&allocation.id)
        .map_err(|_| AllocationError::Corrupt)?;
    Ok(format!("channel-{}", &digest[..32]))
}

impl ReuseWorkspaceCreation {
    pub fn require(
        &self,
        snapshot: &ChannelReuseSnapshot,
        workspace: &Workspace,
    ) -> Result<(), AllocationError> {
        let AllocationPlacement::ReuseParent {
            target,
            profiles,
            parent,
        } = &snapshot.allocation.placement
        else {
            return Err(AllocationError::IdentityMismatch);
        };
        if parent.workspace_created_at.is_some()
            || self.schema_version != 1
            || self.allocation_id != snapshot.allocation.id
            || self.source != snapshot.allocation.first_source
            || self.workspace_id != workspace.id
            || self.workspace_created_at != workspace.created_at
            || self.sandbox_created_at != snapshot.sandbox.created_at
            || self.profile_digest != profiles.workspace.digest
            || workspace.id != target.workspace_id
            || workspace.sandbox_id != target.sandbox_id
            || workspace.owner != snapshot.sandbox.owner
            || workspace.profile != profiles.workspace
            || workspace.name != workspace_name(&snapshot.allocation)?
            || workspace.source != source(&profiles.workspace.content.location)
            || workspace.origin != WorkspaceOrigin::Manual
            || workspace.work.step != WorkspaceWorkStep::None
            || workspace.work.lease.is_some()
            || workspace.work.execution.is_some()
            || workspace.removed_at.is_some()
            || !matches!(
                workspace.status,
                WorkspaceStatus::Preparing | WorkspaceStatus::Ready
            )
        {
            return Err(AllocationError::IdentityMismatch);
        }
        workspace
            .validate_for_admission(&snapshot.sandbox, snapshot.sandbox.status)
            .map_err(|_| AllocationError::Corrupt)
    }
}

pub(crate) fn workspace(
    snapshot: &ChannelReuseSnapshot,
) -> Result<(Workspace, ReuseWorkspaceCreation), AllocationError> {
    let AllocationPlacement::ReuseParent {
        target,
        profiles,
        parent,
    } = &snapshot.allocation.placement
    else {
        return Err(AllocationError::IdentityMismatch);
    };
    if parent.workspace_created_at.is_some()
        || !snapshot.sandbox.credential.delivered
        || snapshot.sandbox.status != SandboxStatus::Active
    {
        return Err(AllocationError::InvalidTransition);
    }
    let name = workspace_name(&snapshot.allocation)?;
    let stamp = layer2_core::rfc3339_at(std::time::SystemTime::now());
    let workspace = Workspace {
        id: target.workspace_id.clone(),
        sandbox_id: target.sandbox_id.clone(),
        owner: snapshot.sandbox.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: profiles.workspace.clone(),
        path: format!("/home/cma/{name}"),
        name,
        source: source(&profiles.workspace.content.location),
        resolved_commit: None,
        status: WorkspaceStatus::Preparing,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, stamp.clone()),
        next_worktree_number: 1,
        created_at: stamp,
        ready_at: None,
        failed: None,
        removed_at: None,
        revision: 1,
    };
    let creation = ReuseWorkspaceCreation {
        schema_version: 1,
        allocation_id: snapshot.allocation.id.clone(),
        source: snapshot.allocation.first_source.clone(),
        workspace_id: workspace.id.clone(),
        workspace_created_at: workspace.created_at.clone(),
        sandbox_created_at: snapshot.sandbox.created_at.clone(),
        profile_digest: profiles.workspace.digest.clone(),
    };
    creation.require(snapshot, &workspace)?;
    Ok((workspace, creation))
}

pub(crate) fn agent(
    authority: &AllocationAuthority,
    allocation: &AllocationRecord,
    sandbox: &Sandbox,
    workspace: &Workspace,
    capabilities: layer2_core::RuntimeCapabilities,
) -> Result<(Worktree, Agent, AgentCreationProvenance, AgentLogEntry), AllocationError> {
    authority.require_record(allocation)?;
    let AllocationPlacement::ReuseParent {
        target, profiles, ..
    } = &allocation.placement
    else {
        return Err(AllocationError::IdentityMismatch);
    };
    if workspace.id != target.workspace_id
        || workspace.profile != profiles.workspace
        || workspace.next_worktree_number == 0
        || workspace.status != WorkspaceStatus::Ready
    {
        return Err(AllocationError::IdentityMismatch);
    }
    let commit = workspace
        .resolved_commit
        .as_ref()
        .ok_or(AllocationError::InvalidTransition)?;
    if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(AllocationError::Corrupt);
    }
    let worktree = Worktree {
        id: target.worktree_id.clone(),
        workspace_id: workspace.id.clone(),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        name: format!("wt-{}", workspace.next_worktree_number),
        path: format!(
            "{}/.worktrees/wt-{}",
            workspace.path, workspace.next_worktree_number
        ),
        branch: format!("cma/wt-{}", workspace.next_worktree_number),
        base_commit: commit.clone(),
        status: WorktreeStatus::Creating,
        agent_id: Some(target.agent_id.clone()),
        revision: 1,
    };
    let at = layer2_core::rfc3339_at(std::time::SystemTime::now());
    let agent = Agent {
        id: target.agent_id.clone(),
        worktree_id: worktree.id.clone(),
        workspace_id: workspace.id.clone(),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        creation_key: format!("channel-allocation:{}", allocation.id),
        creation_fingerprint: channel_fingerprint(allocation)
            .map_err(|_| AllocationError::Corrupt)?,
        profile: profiles.agent.clone(),
        capabilities,
        status: AgentStatus::Starting,
        conversation: ConversationState {
            conversation_id: None,
            runtime_generation: None,
            last_seq: 1,
            first_message: None,
            active_turn: None,
            operation: None,
            queued_messages: Vec::new(),
        },
        work: AgentWorkState::new(AgentWorkStep::None, at.clone()),
        failure: None,
        title: None,
        created_at: at.clone(),
        updated_at: at.clone(),
        ready_at: None,
        stopped_at: None,
        requested_stop_reason: None,
        stop_reason: None,
        deleted_at: None,
        origin: AgentOrigin::Manual,
        revision: 1,
    };
    worktree
        .validate_prerequisites(workspace)
        .map_err(|_| AllocationError::Corrupt)?;
    agent
        .validate_for_admission(&worktree)
        .map_err(|_| AllocationError::Corrupt)?;
    let provenance =
        AgentCreationProvenance::for_channel_admission(authority, allocation, &agent, sandbox)
            .map_err(|_| AllocationError::IdentityMismatch)?;
    let log = AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: 1,
        appended_at: at,
        kind: AgentLogKind::Created {
            workspace_id: workspace.id.clone(),
            worktree_id: worktree.id.clone(),
        },
    };
    Ok((worktree, agent, provenance, log))
}

fn source(location: &WorkspaceLocation) -> WorkspaceSource {
    match location {
        WorkspaceLocation::GitRepository { url, reference } => WorkspaceSource::GitRepository {
            url: url.clone(),
            reference: reference.clone(),
        },
        WorkspaceLocation::Directory { .. } => WorkspaceSource::Empty,
    }
}
