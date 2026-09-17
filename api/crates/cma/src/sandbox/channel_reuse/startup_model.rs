use super::state::ChannelReuseSnapshot;
use crate::sandbox::{
    channel_provisioning::state::ChannelProvisioningPhase,
    channel_startup::model::ChannelStartupPlan,
};
use layer2_core::{
    bridge::allocation::{AllocationAuthority, AllocationError},
    sandbox::{
        Agent, AgentCreationProvenance, AgentStatus, AgentWorkStep, Workspace, WorkspaceStatus,
        Worktree, WorktreeStatus,
    },
};
use layer3_adapter::{AgentOpenRequest, WorktreeReceipt, WorktreeRequest};

#[derive(Clone)]
pub(crate) struct ChannelReuseStartupContext {
    pub snapshot: ChannelReuseSnapshot,
    pub workspace: Workspace,
    pub worktree: Worktree,
    pub agent: Agent,
    pub creation: AgentCreationProvenance,
    pub plan: Option<ChannelStartupPlan>,
}

impl ChannelReuseStartupContext {
    pub fn require(&self, authority: &AllocationAuthority) -> Result<(), AllocationError> {
        let snapshot = &self.snapshot;
        snapshot.require(authority)?;
        authority.require_record(&snapshot.allocation)?;
        let target = snapshot.allocation.placement.target();
        if snapshot.state.phase != ChannelProvisioningPhase::AgentStart
            || self.workspace.id != target.workspace_id
            || self.workspace.sandbox_id != snapshot.sandbox.id
            || self.workspace.status != WorkspaceStatus::Ready
            || self.worktree.id != target.worktree_id
            || self.worktree.workspace_id != self.workspace.id
            || self.worktree.sandbox_id != snapshot.sandbox.id
            || self.worktree.status != WorktreeStatus::Creating
            || self.agent.id != target.agent_id
            || self.agent.worktree_id != self.worktree.id
            || self.agent.workspace_id != self.workspace.id
            || self.agent.sandbox_id != snapshot.sandbox.id
            || self.agent.status != AgentStatus::Starting
            || self.agent.work.step != AgentWorkStep::None
            || self.agent.work.lease.is_some()
            || self.agent.conversation.conversation_id.is_some()
            || self.agent.conversation.runtime_generation.is_some()
            || self.agent.conversation.last_seq != 1
            || self.agent.requested_stop_reason.is_some()
            || self.workspace.owner != snapshot.sandbox.owner
            || self.worktree.owner != snapshot.sandbox.owner
            || self.agent.owner != snapshot.sandbox.owner
            || self.workspace.resolved_commit.as_ref() != Some(&self.worktree.base_commit)
            || self.worktree.agent_id.as_ref() != Some(&self.agent.id)
            || !self.agent.work.receipts.is_empty()
            || self.agent.work.warm_conversation_id.is_some()
            || self.agent.work.cleanup_not_before.is_some()
        {
            return Err(AllocationError::IdentityMismatch);
        }
        self.creation
            .validate_against(&self.agent, &snapshot.sandbox)
            .map_err(|_| AllocationError::IdentityMismatch)?;
        let layer2_core::bridge::allocation::AllocationPlacement::ReuseParent { profiles, .. } =
            &snapshot.allocation.placement
        else {
            return Err(AllocationError::IdentityMismatch);
        };
        let expected_origin = layer2_core::sandbox::AgentCreationOrigin::ChannelBridge {
            bridge_id: snapshot.allocation.identity.bridge_id.as_str().to_owned(),
            partition_id: snapshot.allocation.identity.key.hash().to_owned(),
            generation: snapshot.allocation.identity.generation,
            source_reference: snapshot.allocation.first_source.inbox_id().to_owned(),
        };
        // The immutable creation record survives startup-intent revisions.
        // The admission constructor is deliberately restricted to revision 1.
        if self.creation.creation_origin != expected_origin
            || self.creation.owner != snapshot.allocation.owner
            || self.creation.created_by.as_ref() != Some(&profiles.approved_by)
            || self.agent.creation_key != format!("channel-allocation:{}", snapshot.allocation.id)
            || self.agent.creation_fingerprint
                != layer2_core::sandbox::channel_fingerprint(&snapshot.allocation)
                    .map_err(|_| AllocationError::IdentityMismatch)?
            || self.agent.profile != profiles.agent
            || snapshot.sandbox.profile != profiles.sandbox
        {
            return Err(AllocationError::IdentityMismatch);
        }
        if let Some(plan) = &self.plan {
            self.require_plan(plan)?;
            if self.agent.work.resolved_skills.as_ref() != Some(&plan.skills)
                || self.agent.work.cleanup_debt != plan.intents()
            {
                return Err(AllocationError::IdentityMismatch);
            }
        } else if self.agent.work.resolved_skills.is_some()
            || !self.agent.work.cleanup_debt.is_empty()
            || self.agent.work.next_fence != 1
        {
            return Err(AllocationError::IdentityMismatch);
        }
        Ok(())
    }

    pub fn require_plan(&self, plan: &ChannelStartupPlan) -> Result<(), AllocationError> {
        let options =
            crate::sandbox::runtime_options(&self.agent, &self.snapshot.sandbox, &self.worktree);
        let worktree = WorktreeRequest::new(
            format!(
                "worktree:{}:{}:add:v1",
                self.workspace.name, self.worktree.name
            ),
            self.workspace.name.clone(),
            self.worktree.name.clone(),
            self.worktree.base_commit.clone(),
        );
        let open = AgentOpenRequest::new(
            format!("agent:{}:open:v1", self.agent.id),
            options.clone(),
            None,
        );
        if plan.allocation_id != self.snapshot.allocation.id
            || plan.worktree != worktree
            || plan.open != open
            || plan.home.operation_id != format!("agent:{}:home:create:v1", self.agent.id)
            || !plan.home.has_valid_sha256()
            || plan.home.options != options
            || plan.home.system_prompt
                != crate::sandbox::channel_startup::prompt::system_prompt(
                    plan.schema_version,
                    &self.agent.profile.content.system_prompt,
                )?
            || plan.skills.len() != self.agent.profile.content.skills.len()
            || plan.home.skills.len() != plan.skills.len()
            || !plan
                .skills
                .iter()
                .zip(&self.agent.profile.content.skills)
                .all(|(skill, reference)| {
                    skill.validate().is_ok()
                        && skill.id == reference.id
                        && skill.source == reference.source
                        && reference
                            .version
                            .as_ref()
                            .is_none_or(|version| version == &skill.version)
                })
            || !plan
                .home
                .skills
                .iter()
                .zip(&plan.skills)
                .all(|(runtime, skill)| {
                    runtime.id == skill.id.as_str()
                        && runtime.archive_sha256 == skill.archive_sha256
                        && runtime.version.as_ref() == Some(&skill.version)
                        && runtime.name == skill.name
                        && runtime.source_url.len() <= 2048
                        && reqwest::Url::parse(&runtime.source_url).is_ok_and(|url| {
                            url.scheme() == "https"
                                && url.host_str().is_some()
                                && url.username().is_empty()
                                && url.password().is_none()
                                && url.fragment().is_none()
                        })
                })
        {
            return Err(AllocationError::IdentityMismatch);
        }
        Ok(())
    }

    pub fn valid_worktree(&self, plan: &ChannelStartupPlan, receipt: &WorktreeReceipt) -> bool {
        receipt.operation_id == plan.worktree.operation_id
            && receipt.request_sha256 == plan.worktree.request_sha256
            && receipt.workspace == self.workspace.name
            && receipt.name == self.worktree.name
            && receipt.path == self.worktree.path
            && receipt.branch == self.worktree.branch
            && receipt.base_commit == self.worktree.base_commit
    }
}
