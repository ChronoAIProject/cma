use crate::sandbox::{
    Agent, AgentStatus, Sandbox, SandboxImageRotationPhase, SandboxLifecycleOperation,
    SandboxRevivePhase, SandboxWorkStep,
};
use crate::seconds_from_rfc3339;

use super::{SandboxMutationGuard, SandboxStoreError};

/// Authority for recording replacement output before publishing a reopened Agent.
/// Stores must validate and write-fence the parent in the checkpoint transaction.
#[derive(Debug, Clone)]
pub struct AgentReopenGuard {
    pub sandbox: SandboxMutationGuard,
    pub agent_fence: u64,
    pub native_conversation_id: String,
    pub runtime_generation: u64,
}

impl AgentReopenGuard {
    pub fn project(&self, agent: &Agent) -> Result<Agent, SandboxStoreError> {
        if agent.sandbox_id != self.sandbox.sandbox_id
            || agent.owner != self.sandbox.owner
            || agent.status != AgentStatus::Paused
            || agent.work.next_fence != self.agent_fence
            || agent.work.lease.is_some()
            || agent.work.step.is_pending()
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
        let expires = parent
            .work
            .lease
            .as_ref()
            .and_then(|lease| seconds_from_rfc3339(&lease.expires_at));
        let now = seconds_from_rfc3339(at);
        if SandboxMutationGuard::capture(parent) != self.sandbox
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
