//! Domain transition for a restart after the store proves the exact owner Stop.
use super::{
    Agent, AgentLogKind, AgentStatus, AgentStopReason, AgentWorkStep, SandboxModelError, Worktree,
};
use crate::seconds_from_rfc3339;

impl Agent {
    /// Reserve the live slot and schedule reopening the retained conversation.
    /// The control store must commit this alongside predecessor and admission
    /// evidence. Only the worker can verify attachment and publish Ready.
    pub fn admit_owner_restart(
        &self,
        worktree: &Worktree,
        at: String,
    ) -> Result<Self, SandboxModelError> {
        if self.conversation.active_turn.is_some() {
            return Err(SandboxModelError::invalid_value("agent.restart"));
        }
        self.plan_owner_restart(worktree, at)
            .map(|(agent, _)| agent)
    }

    /// Plan the same restart for records whose original Stop left a turn open.
    /// The store must prove the exact owner Stop and native Close, then append
    /// the interruption and admit the restart together under the same fence.
    pub fn plan_owner_restart(
        &self,
        worktree: &Worktree,
        at: String,
    ) -> Result<(Self, Option<AgentLogKind>), SandboxModelError> {
        self.validate_prerequisites(worktree)?;
        let invalid = || SandboxModelError::invalid_value("agent.restart");
        let requested_at = seconds_from_rfc3339(&at).ok_or_else(invalid)?;
        let stopped_at = self.stopped_at.as_deref().and_then(seconds_from_rfc3339);
        let updated_at = seconds_from_rfc3339(&self.updated_at).ok_or_else(invalid)?;
        if self.status != AgentStatus::Stopped
            || self.stop_reason != Some(AgentStopReason::Owner)
            || self.requested_stop_reason.is_some()
            || stopped_at.is_none_or(|stopped_at| stopped_at > updated_at)
            || updated_at > requested_at
            || self.ready_at.is_none()
            || self.deleted_at.is_some()
            || self.failure.is_some()
            || !self.conversation.queued_messages.is_empty()
            || self
                .conversation
                .first_message
                .as_ref()
                .is_some_and(|message| !message.is_terminal())
            || self
                .conversation
                .operation
                .as_ref()
                .is_some_and(super::AgentOperationRecord::blocks_dispatch)
            || self.work.step != AgentWorkStep::None
            || self.work.lease.is_some()
            || self.work.cleanup_not_before.is_some()
            || !self.work.cleanup_debt.is_empty()
        {
            return Err(invalid());
        }
        let mut next = self.clone();
        let interruption = next.conversation.interrupt_active_turn();
        if interruption.is_some() {
            next.conversation.last_seq = next
                .conversation
                .last_seq
                .checked_add(1)
                .ok_or_else(invalid)?;
        }
        next.revision = next.revision.checked_add(1).ok_or_else(invalid)?;
        next.status = AgentStatus::Starting;
        next.stop_reason = None;
        next.stopped_at = None;
        next.updated_at.clone_from(&at);
        next.work.schedule(AgentWorkStep::RestartConversation, at);
        if interruption.is_some() {
            next.validate_conversation()?;
        }
        Ok((next, interruption))
    }
}
