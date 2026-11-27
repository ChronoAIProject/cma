use super::*;
use crate::sandbox::{
    AgentOperationDelivery, AgentOperationIntent, AgentStatus, ConversationState, SandboxStatus,
};

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
    let conversation = &agent.conversation;
    let updated = &updated_agent.conversation;
    if conversation.active_turn == updated.active_turn && conversation.last_seq <= updated.last_seq
    {
        // The recorder commits a new usage checkpoint together with a log
        // entry. It does not alter the authority or the target of an append.
        let previous_usage = conversation.observations.usage.as_ref();
        let usage_advanced = conversation.last_seq < updated.last_seq
            && updated.observations.usage.as_ref().is_some_and(|usage| {
                Some(&usage.conversation_id) == updated.conversation_id.as_ref()
                    && previous_usage.is_none_or(|previous| {
                        previous.input_tokens <= usage.input_tokens
                            && previous.output_tokens <= usage.output_tokens
                    })
                    && match conversation.active_turn {
                        Some(sequence) => usage
                            .current_turn
                            .as_ref()
                            .is_some_and(|turn| turn.turn_sequence == sequence),
                        // An idle usage fold appends one usage entry and leaves
                        // the last turn's accounting intact.
                        None => {
                            conversation.last_seq.checked_add(1) == Some(updated.last_seq)
                                && previous_usage != Some(usage)
                                && previous_usage
                                    .and_then(|previous| previous.current_turn.as_ref())
                                    == usage.current_turn.as_ref()
                        }
                    }
            });
        if conversation.active_turn.is_some() || usage_advanced {
            comparable_agent.conversation.last_seq = conversation.last_seq;
        }
        if usage_advanced {
            comparable_agent
                .conversation
                .observations
                .usage
                .clone_from(&conversation.observations.usage);
        }
        // Observation of the already-written message advances delivery on
        // this turn without replacing its operation or changing the target.
        if conversation.last_seq < updated.last_seq
            && observes_confirmed_send(conversation, updated)
        {
            comparable_agent
                .conversation
                .operation
                .clone_from(&conversation.operation);
        }
    }
    let mut comparable_sandbox = next.sandbox.clone();
    comparable_sandbox.revision = previous.sandbox.revision;
    comparable_sandbox
        .updated_at
        .clone_from(&previous.sandbox.updated_at);
    &comparable_agent == agent && comparable_sandbox == previous.sandbox
}

fn observes_confirmed_send(previous: &ConversationState, next: &ConversationState) -> bool {
    let (Some(operation), Some(observed)) = (&previous.operation, &next.operation) else {
        return false;
    };
    if !matches!(operation.intent, AgentOperationIntent::SendMessage { .. })
        || previous.active_turn != Some(operation.turn_sequence)
        || previous.runtime_generation != Some(operation.runtime_generation)
        || operation.state != AgentOperationDelivery::WriteConfirmed
        || observed.state != AgentOperationDelivery::ExecutionObserved
    {
        return false;
    }
    let Some(at) = &observed.execution_observed_at else {
        return false;
    };
    let mut expected = operation.clone();
    expected.mark_execution_observed(at.clone()).is_ok() && &expected == observed
}
