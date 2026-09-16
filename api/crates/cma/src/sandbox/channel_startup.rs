//! Real runtime startup precedes the ordinary connection-publication handoff.
pub(crate) mod model;
#[cfg(test)]
pub(crate) mod test_support;
use super::{
    agent_skills::{AgentSkillResolver, OrnnAgentSkillResolver},
    channel_provisioning::{state::ChannelProvisioningSnapshot, ChannelControlPlaneVerifier},
};
use crate::{bridge::BridgeControlPlaneLease, store::MongoBridgeAllocationStore};
use layer2_core::bridge::allocation::{AllocationAuthority, AllocationError, AllocationRecord};
use layer3_adapter::{AgentHomeRequest, AgentOpenRequest, SandboxAdapter, WorktreeRequest};
use model::ChannelStartupPlan;
use std::sync::Arc;

pub(crate) struct ChannelStartupDriver {
    pub allocations: Arc<MongoBridgeAllocationStore>,
    pub credentials: Arc<dyn ChannelControlPlaneVerifier>,
    pub adapter: Arc<dyn SandboxAdapter>,
    pub skills: Arc<OrnnAgentSkillResolver>,
    pub resources: Arc<crate::store::MongoSandboxStore>,
}

impl ChannelStartupDriver {
    pub(crate) async fn advance(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        snapshot: &ChannelProvisioningSnapshot,
    ) -> Result<AllocationRecord, AllocationError> {
        self.credentials
            .revalidate_control_plane(lease)
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        let mut context = self
            .allocations
            .channel_startup_context(authority, lease, snapshot)
            .await?;
        let plan = if let Some(plan) = &context.plan {
            plan.clone()
        } else {
            let material = self
                .skills
                .resolve_public_with_credential(
                    lease.credential(),
                    &context.agent.profile.content.skills,
                )
                .await
                .map_err(|_| AllocationError::AuthorityUnavailable)?;
            let skills = self
                .skills
                .runtime_skills(&material)
                .map_err(|_| AllocationError::InvalidProfiles)?;
            let options =
                super::runtime_options(&context.agent, &snapshot.sandbox, &context.worktree);
            ChannelStartupPlan {
                schema_version: 1,
                allocation_id: snapshot.allocation.id.clone(),
                skills: material,
                worktree: WorktreeRequest::new(
                    format!(
                        "worktree:{}:{}:add:v1",
                        context.workspace.name, context.worktree.name
                    ),
                    context.workspace.name.clone(),
                    context.worktree.name.clone(),
                    context.worktree.base_commit.clone(),
                ),
                home: AgentHomeRequest::new(
                    format!("agent:{}:home:create:v1", context.agent.id),
                    options.clone(),
                    context.agent.profile.content.system_prompt.clone(),
                    skills,
                ),
                open: AgentOpenRequest::new(
                    format!("agent:{}:open:v1", context.agent.id),
                    options,
                    None,
                ),
            }
        };
        context.require_plan(&plan)?;
        context = self
            .allocations
            .prepare_channel_startup(authority, lease, &context, &plan)
            .await?;
        let runtime = super::runtime_attachment::adopt_bound_runtime(
            self.adapter.as_ref(),
            &snapshot.sandbox,
            &snapshot.state.settings.deployment_id,
        )
        .await
        .map_err(|_| AllocationError::Unavailable)?;
        let fence = snapshot
            .allocation
            .lease
            .as_ref()
            .ok_or(AllocationError::LeaseUnavailable)?
            .fence;
        self.revalidate(authority, lease, &context).await?;
        let worktree = self
            .adapter
            .add_worktree(&runtime, fence, &plan.worktree)
            .await
            .map_err(|_| AllocationError::Unavailable)?;
        if !context.valid_worktree(&plan, &worktree) {
            return Err(AllocationError::IdentityMismatch);
        }
        self.revalidate(authority, lease, &context).await?;
        self.adapter
            .materialize_agent_home(&runtime, fence, &plan.home)
            .await
            .map_err(|_| AllocationError::Unavailable)?;
        self.revalidate(authority, lease, &context).await?;
        let (updates, mut receiver) = tokio::sync::mpsc::channel(256);
        let opened = self
            .adapter
            .open_agent(&runtime, fence, &plan.open, updates);
        tokio::pin!(opened);
        let mut buffered = 0;
        let mut updates_open = true;
        let channel = loop {
            tokio::select! {
                result = &mut opened => break result.map_err(|_| AllocationError::Unavailable)?,
                update = receiver.recv(), if updates_open => {
                    if update.is_none() { updates_open = false; continue; }
                    buffered += 1;
                    if buffered > 256 { return Err(AllocationError::Unavailable); }
                }
                _ = tokio::time::sleep_until(lease.deadline()) => return Err(AllocationError::AuthorityUnavailable),
            }
        };
        let conversation = channel
            .conversation_id
            .as_deref()
            .filter(|id| {
                !id.trim().is_empty() && id.len() <= 512 && !id.chars().any(char::is_control)
            })
            .ok_or(AllocationError::IdentityMismatch)?;
        self.revalidate(authority, lease, &context).await?;
        // Only this genuine open authorizes Ready. PublishConnection then
        // reconnects this same canonical request through the existing registry.
        self.allocations
            .finish_channel_startup(
                authority,
                lease,
                &context,
                &plan,
                &worktree,
                conversation,
                &self.resources,
            )
            .await
    }

    async fn revalidate(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        context: &model::ChannelStartupContext,
    ) -> Result<(), AllocationError> {
        self.credentials
            .revalidate_control_plane(lease)
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        let current = self
            .allocations
            .channel_startup_context(authority, lease, &context.snapshot)
            .await?;
        if current.agent != context.agent
            || current.worktree != context.worktree
            || current.workspace != context.workspace
            || current.plan != context.plan
        {
            return Err(AllocationError::Conflict);
        }
        Ok(())
    }
}
