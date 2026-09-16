use super::*;
use crate::sandbox::{
    channel_provisioning::state::ChannelProvisioningPhase,
    channel_reuse::state::ChannelReuseSnapshot,
};
use layer2_core::{
    bridge::allocation::{AllocationPlacement, AllocationState},
    sandbox::{AgentStatus, AgentWorkStep, WorkspaceStatus, WorktreeStatus},
};

impl ChannelReuseCleanupSeal {
    pub(crate) fn validate(&self) -> Result<(), AllocationError> {
        let hash = |value: &str| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        if !self.allocation_id.strip_prefix("bral_").is_some_and(hash)
            || ![
                &self.allocation_sha256,
                &self.creation_sha256,
                &self.startup_sha256,
            ]
            .iter()
            .all(|value| hash(value))
            || self
                .workspace_creation_sha256
                .as_ref()
                .is_some_and(|value| !hash(value))
            || self.workspace_owned != self.workspace_creation_sha256.is_some()
            || [
                &self.sandbox_created_at,
                &self.workspace_created_at,
                &self.agent_created_at,
            ]
            .iter()
            .any(|stamp| layer2_core::seconds_from_rfc3339(stamp).is_none())
            || self
                .runtime
                .provision_generation
                .is_none_or(|generation| generation == 0)
            || self
                .runtime
                .boot_id
                .as_ref()
                .is_none_or(|value| value.is_empty())
            || self.runtime.runtime_id.is_empty()
            || self.runtime.runtime_version.is_empty()
            || layer2_core::seconds_from_rfc3339(&self.runtime.bound_at).is_none()
            || self.home_path != format!("/home/cma/.cma/agents/{}/home", self.target.agent_id)
            || !self
                .worktree_path
                .strip_prefix(&format!("{}/.worktrees/wt-", self.workspace_path))
                .is_some_and(|suffix| {
                    suffix
                        .parse::<u64>()
                        .is_ok_and(|number| number > 0 && suffix == number.to_string())
                })
            || self
                .workspace_path
                .strip_prefix("/home/cma/")
                .is_none_or(|name| name.is_empty() || name.contains('/'))
        {
            return Err(AllocationError::Corrupt);
        }
        for path in [&self.workspace_path, &self.worktree_path, &self.home_path] {
            require_path(path)?;
        }
        Ok(())
    }
    pub(crate) fn from_snapshot(snapshot: &ChannelReuseSnapshot) -> Result<Self, AllocationError> {
        snapshot.require_ownership()?;
        let allocation = &snapshot.allocation;
        let AllocationPlacement::ReuseParent { parent, target, .. } = &allocation.placement else {
            return Err(AllocationError::IdentityMismatch);
        };
        let workspace = snapshot
            .workspace
            .as_ref()
            .ok_or(AllocationError::Corrupt)?;
        let agent = snapshot.agent.as_ref().ok_or(AllocationError::Corrupt)?;
        let tree = snapshot.worktree.as_ref().ok_or(AllocationError::Corrupt)?;
        let creation = snapshot.creation.as_ref().ok_or(AllocationError::Corrupt)?;
        let plan = snapshot
            .state
            .startup_plan
            .as_ref()
            .ok_or(AllocationError::Corrupt)?;
        let runtime = snapshot
            .sandbox
            .runtime
            .as_ref()
            .ok_or(AllocationError::AuthorityUnavailable)?;
        if allocation.state != AllocationState::Retired
            || snapshot.state.phase != ChannelProvisioningPhase::Ready
            || allocation
                .receipt
                .as_ref()
                .is_none_or(|receipt| receipt.target != *target)
            || workspace.status != WorkspaceStatus::Ready
            || workspace.path != format!("/home/cma/{}", workspace.name)
            || tree.status != WorktreeStatus::Ready
            || tree.agent_id.as_ref() != Some(&agent.id)
            || agent.deleted_at.is_some()
            || !agent.status.is_live()
            || agent.ready_at.is_none()
            || agent.conversation.conversation_id.is_none()
            || agent.conversation.runtime_generation.is_none()
            || parent.sandbox_created_at != snapshot.sandbox.created_at
            || parent
                .workspace_created_at
                .as_ref()
                .is_some_and(|stamp| stamp != &workspace.created_at)
            || !plan.worktree.has_valid_sha256()
            || !plan.home.has_valid_sha256()
            || !plan.open.has_valid_sha256()
            || plan.worktree.operation_id
                != format!("worktree:{}:{}:add:v1", workspace.name, tree.name)
            || plan.worktree.workspace != workspace.name
            || plan.worktree.name != tree.name
            || plan.worktree.base != tree.base_commit
            || plan.home.operation_id != format!("agent:{}:home:create:v1", agent.id)
            || plan.open.operation_id != format!("agent:{}:open:v1", agent.id)
            || plan.open.resume_conversation.is_some()
            || plan.home.options != plan.open.options
            || plan.home.options.slot.as_str() != agent.id.as_str()
            || plan.home.options.working_directory != tree.path
            || plan.home.options.home_directory
                != format!("/home/cma/.cma/agents/{}/home", agent.id)
            || runtime
                .provision_generation
                .is_none_or(|generation| generation == 0)
            || runtime.boot_id.as_ref().is_none_or(|id| id.is_empty())
        {
            return Err(AllocationError::IdentityMismatch);
        }
        require_path(&workspace.path)?;
        require_path(&tree.path)?;
        require_path(&plan.home.options.home_directory)?;
        let seal = Self {
            allocation_id: allocation.id.clone(),
            owner: allocation.owner.clone(),
            target: target.clone(),
            sandbox_created_at: snapshot.sandbox.created_at.clone(),
            workspace_created_at: workspace.created_at.clone(),
            agent_created_at: agent.created_at.clone(),
            runtime: runtime.clone(),
            workspace_path: workspace.path.clone(),
            worktree_path: tree.path.clone(),
            home_path: plan.home.options.home_directory.clone(),
            workspace_owned: parent.workspace_created_at.is_none(),
            allocation_sha256: digest(allocation)?,
            creation_sha256: digest(creation)?,
            startup_sha256: digest(plan)?,
            workspace_creation_sha256: snapshot
                .workspace_creation
                .as_ref()
                .map(digest)
                .transpose()?,
        };
        seal.validate()?;
        Ok(seal)
    }
}

pub(crate) fn agent_busy(snapshot: &ChannelReuseSnapshot) -> bool {
    snapshot.agent.as_ref().is_none_or(|agent| {
        agent.status != AgentStatus::Ready
            || agent.work.step != AgentWorkStep::None
            || agent.work.lease.is_some()
            || !agent.work.cleanup_debt.is_empty()
            || agent.conversation.operation.is_some()
            || agent.conversation.active_turn.is_some()
            || !agent.conversation.queued_messages.is_empty()
            || agent.requested_stop_reason.is_some()
    })
}

fn require_path(path: &str) -> Result<(), AllocationError> {
    if !path.starts_with("/home/cma/")
        || path.as_bytes().contains(&0)
        || path
            .split('/')
            .skip(1)
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(AllocationError::IdentityMismatch);
    }
    Ok(())
}
