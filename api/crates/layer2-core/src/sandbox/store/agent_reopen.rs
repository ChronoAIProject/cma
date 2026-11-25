use crate::sandbox::{
    Agent, AgentStatus, AgentWorkLease, AgentWorkStep, Sandbox, SandboxImageRotationPhase,
    SandboxLifecycleOperation, SandboxRevivePhase, SandboxStatus, SandboxWorkStep,
};
use crate::seconds_from_rfc3339;

use super::{SandboxMutationGuard, SandboxStoreError};

/// The checkpoint observed before dispatch, paired with the owner that
/// returned the refusal. Parking must compare it in the lifecycle transaction.
#[derive(Debug, Clone)]
pub struct ReopenRefusalFence {
    pub agent_id: crate::AgentId,
    pub agent_revision: u64,
    pub checkpoint: Option<Box<super::AgentMessageCheckpoint>>,
    pub owner: Option<crate::agent_ownership::OwnershipToken>,
    pub released: Option<layer3_adapter::ReleasedRuntime>,
}

/// Authority for recording replacement output before publishing a reopened Agent.
/// Stores must validate and write-fence the parent in the checkpoint transaction.
#[derive(Debug, Clone)]
pub struct AgentReopenGuard {
    pub sandbox: SandboxMutationGuard,
    pub agent_fence: u64,
    pub native_conversation_id: String,
    pub runtime_generation: u64,
    pub owner_restart: Option<OwnerRestartOutputFence>,
}

#[derive(Debug, Clone)]
pub struct OwnerRestartOutputFence {
    pub agent_id: crate::AgentId,
    pub restart_operation: String,
    pub open_operation: String,
    pub open_request_sha256: String,
    pub lease: AgentWorkLease,
}

impl AgentReopenGuard {
    pub fn project(&self, agent: &Agent) -> Result<Agent, SandboxStoreError> {
        let exact_work = match &self.owner_restart {
            Some(restart) => {
                agent.id == restart.agent_id
                    && agent.status == AgentStatus::Starting
                    && agent.work.step == AgentWorkStep::RestartConversation
                    && agent.work.lease.as_ref() == Some(&restart.lease)
                    && matches!(agent.work.cleanup_debt.as_slice(), [debt]
                    if debt.step == AgentWorkStep::OpenConversation
                        && debt.operation_id == restart.open_operation
                        && debt.request_sha256 == restart.open_request_sha256)
                    && agent
                        .conversation
                        .runtime_generation
                        .and_then(|prior| prior.checked_add(1))
                        == Some(self.runtime_generation)
            }
            None => {
                agent.status == AgentStatus::Paused
                    && agent.work.lease.is_none()
                    && !agent.work.step.is_pending()
            }
        };
        if agent.sandbox_id != self.sandbox.sandbox_id
            || agent.owner != self.sandbox.owner
            || !exact_work
            || agent.work.next_fence != self.agent_fence
            || agent.requested_stop_reason.is_some()
            || agent.conversation.active_turn.is_some()
            || agent
                .conversation
                .operation
                .as_ref()
                .is_some_and(|op| op.blocks_dispatch())
            || agent.conversation.conversation_id.as_deref() != Some(&self.native_conversation_id)
            || agent
                .conversation
                .runtime_generation
                .is_none_or(|old| old >= self.runtime_generation)
        {
            return Err(SandboxStoreError::Conflict);
        }
        let mut projected = agent.clone();
        projected.conversation.runtime_generation = Some(self.runtime_generation);
        Ok(projected)
    }

    pub fn validate(
        &self,
        parent: &Sandbox,
        agent: &Agent,
        at: &str,
    ) -> Result<(), SandboxStoreError> {
        self.project(agent)?;
        if let Some(restart) = &self.owner_restart {
            let now = seconds_from_rfc3339(at);
            let expires = seconds_from_rfc3339(&restart.lease.expires_at);
            return if self.sandbox.matches(parent)
                && parent.status == SandboxStatus::Active
                && parent.lifecycle.operation.is_none()
                && matches!((now, expires), (Some(now), Some(expires)) if now < expires)
            {
                Ok(())
            } else {
                Err(SandboxStoreError::Conflict)
            };
        }
        let expires = parent
            .work
            .lease
            .as_ref()
            .and_then(|lease| seconds_from_rfc3339(&lease.expires_at));
        let now = seconds_from_rfc3339(at);
        if !self.sandbox.matches(parent)
            || parent.work.step != SandboxWorkStep::RunLifecycleOperation
            || !matches!((now, expires), (Some(now), Some(expires)) if now < expires)
            || parent
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.provision_generation)
                != Some(self.runtime_generation)
            || !matches!(
                parent.lifecycle.operation,
                Some(SandboxLifecycleOperation::Revive {
                    phase: SandboxRevivePhase::ConversationReopen,
                    ..
                }) | Some(SandboxLifecycleOperation::ImageRotation {
                    phase: SandboxImageRotationPhase::ConversationReopen,
                    ..
                })
            )
        {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(())
    }
}
