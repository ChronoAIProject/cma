use super::store::WorkspaceAdmissionKind;
use super::{
    Agent, AgentStatus, AgentWorkStep, FirstMessageDelivery, Sandbox, SandboxEventKind,
    SandboxLifecycleOperation, SandboxModelError, SandboxReviveCause, SandboxStatus,
};

impl WorkspaceAdmissionKind {
    pub fn accepts(self, sandbox: &Sandbox) -> bool {
        match self {
            Self::Manual | Self::Trigger | Self::Lifecycle => sandbox.accepts_creation_admission(),
            Self::SandboxProfile => {
                (sandbox.status == SandboxStatus::Installing
                    || matches!(&sandbox.origin, super::SandboxOrigin::Lifecycle { operation_id }
                        if sandbox.accepts_lifecycle_bootstrap(operation_id)))
                    && sandbox.lifecycle.checkpoint_capture.is_none()
            }
        }
    }
}

impl Sandbox {
    pub fn accepts_creation_admission(&self) -> bool {
        self.lifecycle.checkpoint_capture.is_none()
            && (matches!(self.status, SandboxStatus::Active | SandboxStatus::Paused)
                || (self.status == SandboxStatus::Reviving
                    && matches!(
                        self.lifecycle.operation,
                        Some(SandboxLifecycleOperation::Revive { .. })
                    )))
    }

    /// Persist with the child reservation, after all admission checks succeed.
    pub fn record_creation_admission(
        &mut self,
        cause: SandboxReviveCause,
        at: String,
    ) -> Result<Option<SandboxEventKind>, SandboxModelError> {
        if !self.accepts_creation_admission()
            || !matches!(
                cause,
                SandboxReviveCause::WorkspaceCreation
                    | SandboxReviveCause::AgentCreation
                    | SandboxReviveCause::TriggerLaunch
            )
        {
            return Err(SandboxModelError::invalid_value(
                "sandbox.creation_admission",
            ));
        }
        if self.status == SandboxStatus::Reviving {
            self.cancel_recovery_pause(&at)?;
            return Ok(None);
        }
        let mut next = self.clone();
        let event = if next.status == SandboxStatus::Paused {
            next.begin_revive(cause, at.clone())?;
            Some(SandboxEventKind::StatusChanged {
                from: SandboxStatus::Paused,
                to: SandboxStatus::Reviving,
            })
        } else {
            next.revision = next
                .revision
                .checked_add(1)
                .ok_or_else(|| SandboxModelError::invalid_value("sandbox.revision"))?;
            next.updated_at.clone_from(&at);
            None
        };
        next.last_activity_at = at;
        next.validate()?;
        *self = next;
        Ok(event)
    }

    /// New reservations have no checkpoint content until their first runtime intent.
    pub fn restores_agent(&self, agent: &Agent) -> bool {
        if self.retained_home_recovery().is_some() {
            return agent.status.is_live()
                && agent.status != AgentStatus::Starting
                && agent.requested_stop_reason.is_none();
        }
        agent.status.is_live()
            && !(self.status == SandboxStatus::Reviving
                && self
                    .lifecycle
                    .checkpoints
                    .latest
                    .as_ref()
                    .is_some_and(|checkpoint| {
                        !checkpoint
                            .conversations
                            .iter()
                            .any(|conversation| conversation.agent_id == agent.id)
                    })
                && unstarted_creation(agent))
    }
}

fn unstarted_creation(agent: &Agent) -> bool {
    // Runtime effects always persist debt before I/O; completed effects retain receipts.
    let pending_start = agent.work.step == AgentWorkStep::AddWorktree
        && agent.requested_stop_reason.is_none()
        && agent.work.cleanup_not_before.is_none();
    let pending_stop = matches!(
        agent.work.step,
        AgentWorkStep::FenceInteraction
            | AgentWorkStep::CloseProcess
            | AgentWorkStep::PublishStopped
    ) && agent.requested_stop_reason.is_some();
    agent.status == AgentStatus::Starting
        && (pending_start || pending_stop)
        && agent.work.cleanup_debt.is_empty()
        && agent.work.receipts.is_empty()
        && agent.work.resolved_skills.is_none()
        && agent.conversation.conversation_id.is_none()
        && agent.conversation.runtime_generation.is_none()
        && agent.conversation.active_turn.is_none()
        && agent.conversation.operation.is_none()
        && agent.conversation.queued_messages.is_empty()
        && agent
            .conversation
            .first_message
            .as_ref()
            .is_none_or(|message| message.state == FirstMessageDelivery::Queued)
        && agent.ready_at.is_none()
        && agent.failure.is_none()
        && agent.stopped_at.is_none()
        && agent.stop_reason.is_none()
        && agent.deleted_at.is_none()
}
