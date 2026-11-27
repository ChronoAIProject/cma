use super::*;
use crate::sandbox::{AgentStatus, SandboxStatus};

#[derive(Clone, Copy)]
pub(super) enum RevisionRefresh {
    Disabled,
    Creation,
    Retained,
}

impl RevisionRefresh {
    pub(super) fn capture(
        authorization: &AuthorizedResource,
        submission: &CommandSubmission,
        request: &CommandReservation,
        ordinary_admission: bool,
    ) -> Self {
        if submission.expected_agent_revision.is_some()
            || request.binding.is_some()
            || !matches!(request.envelope.command, AgentCommand::Message { .. })
        {
            return Self::Disabled;
        }
        let lineage = &authorization.snapshot.lineage;
        let Some(agent) = lineage.agent.as_ref() else {
            return Self::Disabled;
        };
        if lineage.workspace.as_ref().is_some_and(|workspace| {
            agent
                .bootstrap_lineage(&lineage.sandbox, workspace)
                .or_else(|| agent.prepared_creation_lineage(&lineage.sandbox, workspace))
                .is_some()
        }) {
            return Self::Creation;
        }
        if ordinary_admission
            && matches!(&request.envelope.command, AgentCommand::Message {
                input, order: AgentInputOrder::Append, ..
            } if input.external_event().is_none())
            && matches!(agent.status, AgentStatus::Ready | AgentStatus::Working)
            && agent.requested_stop_reason.is_none()
            && agent.conversation.conversation_id.is_some()
            && agent.conversation.runtime_generation.is_some()
            && lineage.sandbox.status == SandboxStatus::Active
            && lineage.sandbox.runtime.as_ref().is_some_and(|runtime| {
                runtime
                    .boot_id
                    .as_ref()
                    .is_some_and(|boot| !boot.trim().is_empty())
            })
        {
            return Self::Retained;
        }
        Self::Disabled
    }

    pub(super) fn enabled(self) -> bool {
        !matches!(self, Self::Disabled)
    }

    pub(super) fn code(self) -> &'static str {
        match self {
            Self::Disabled => "revision_refresh_disabled",
            Self::Creation => "creation_revision_conflict",
            Self::Retained => "retained_revision_conflict",
        }
    }

    pub(super) fn matches(
        self,
        previous: &AuthorizedResource,
        next: &AuthorizedResource,
        expected: &CommandReservation,
        fresh: &CommandReservation,
    ) -> bool {
        match self {
            Self::Disabled => false,
            Self::Creation => true,
            Self::Retained => {
                fresh.binding.is_none()
                    && fresh.envelope.authorization == expected.envelope.authorization
                    && same_retained_target(previous, next)
            }
        }
    }
}

fn same_retained_target(previous: &AuthorizedResource, next: &AuthorizedResource) -> bool {
    let previous = &previous.snapshot.lineage;
    let next = &next.snapshot.lineage;
    let (Some(agent), Some(updated_agent)) = (&previous.agent, &next.agent) else {
        return false;
    };
    if updated_agent.revision < agent.revision
        || next.sandbox.revision < previous.sandbox.revision
        || (updated_agent.revision == agent.revision
            && next.sandbox.revision == previous.sandbox.revision)
        || previous.workspace != next.workspace
        || previous.worktree != next.worktree
    {
        return false;
    }
    let mut comparable_agent = updated_agent.clone();
    comparable_agent.revision = agent.revision;
    comparable_agent.updated_at.clone_from(&agent.updated_at);
    // Output can advance the history cursor while the same turn is active.
    // Keep the turn, retained boot, settings and lifecycle in the comparison.
    if agent.conversation.active_turn.is_some()
        && agent.conversation.active_turn == updated_agent.conversation.active_turn
        && agent.conversation.last_seq <= updated_agent.conversation.last_seq
    {
        comparable_agent.conversation.last_seq = agent.conversation.last_seq;
    }
    let mut comparable_sandbox = next.sandbox.clone();
    comparable_sandbox.revision = previous.sandbox.revision;
    comparable_sandbox
        .updated_at
        .clone_from(&previous.sandbox.updated_at);
    &comparable_agent == agent && comparable_sandbox == previous.sandbox
}
