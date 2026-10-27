//! The one permitted paired startup publication. Native preparation ownership
//! must still be proved and write-fenced by the store in the publication transaction.
use crate::sandbox::{Agent, AgentLogKind, AgentOrigin, AgentStatus, AgentWorkStep};

use super::agent_work_evidence_validation::validate_histories;
use super::AgentWorkMutation;

/// Recognize the prepared AddWorktree + MaterializeHome publication shape.
/// This is not an eligibility proof: callers must also validate the complete
/// mutation and transactionally fence its exact native preparation and parents.
pub fn is_prepared_agent_replay_pair(current: &Agent, mutation: &AgentWorkMutation) -> bool {
    mutation.worktree.is_none()
        && mutation.sandbox_event.is_none()
        && mutation.appended_at == mutation.agent.updated_at
        && ((paired_intent(current, &mutation.agent) && mutation.agent_logs.is_empty())
            || (paired_completion(current, &mutation.agent) && paired_logs(mutation)))
}

pub(super) fn paired_logs(mutation: &AgentWorkMutation) -> bool {
    matches!(
        mutation.agent_logs.as_slice(),
        [
            AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::AddWorktree
            },
            AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::MaterializeHome
            },
        ]
    )
}

pub(super) fn paired_intent(current: &Agent, next: &Agent) -> bool {
    if !prepared_shape(current, next)
        || !current.work.cleanup_debt.is_empty()
        || !exact_pair(next.work.cleanup_debt.iter().map(|debt| debt.step))
    {
        return false;
    }
    let mut expected = current.work.clone();
    expected.cleanup_debt.clone_from(&next.work.cleanup_debt);
    next.work == expected
}

pub(super) fn paired_completion(current: &Agent, next: &Agent) -> bool {
    if !prepared_shape(current, next)
        || !exact_pair(current.work.cleanup_debt.iter().map(|debt| debt.step))
        || !next.work.cleanup_debt.is_empty()
        || !exact_pair(next.work.receipts.iter().map(|receipt| receipt.step))
        || !current
            .work
            .cleanup_debt
            .iter()
            .zip(&next.work.receipts)
            .all(|(debt, receipt)| {
                debt.step == receipt.step
                    && debt.operation_id == receipt.operation_id
                    && debt.request_sha256 == receipt.request_sha256
                    && receipt.completed_at == next.updated_at
            })
    {
        return false;
    }
    let mut expected = current.work.clone();
    expected.step = AgentWorkStep::OpenConversation;
    expected.ready_at.clone_from(&next.updated_at);
    expected.attempts = 0;
    expected.lease = None;
    expected.cleanup_debt.clear();
    expected.receipts.clone_from(&next.work.receipts);
    next.work == expected
}

fn prepared_shape(current: &Agent, next: &Agent) -> bool {
    current.status == AgentStatus::Starting
        && next.status == AgentStatus::Starting
        && matches!(current.origin, AgentOrigin::Lifecycle { .. })
        && current.work.step == AgentWorkStep::AddWorktree
        && current.work.receipts.is_empty()
        && current.work.cleanup_not_before.is_none()
        && current.work.live_skills.is_empty()
        && current.work.warm_conversation_id.is_some()
        && current.profile.content.skills.is_empty()
        && current
            .work
            .resolved_skills
            .as_ref()
            .is_some_and(Vec::is_empty)
        && current.conversation.conversation_id.is_none()
        && current.conversation.runtime_generation.is_none()
        && next.conversation.conversation_id.is_none()
        && next.conversation.runtime_generation.is_none()
        && current
            .work
            .validate_resolved_skills(&current.profile.content.skills)
            .is_ok()
        && validate_histories(current, next, &next.updated_at).is_ok()
}

fn exact_pair(mut steps: impl Iterator<Item = AgentWorkStep>) -> bool {
    steps.next() == Some(AgentWorkStep::AddWorktree)
        && steps.next() == Some(AgentWorkStep::MaterializeHome)
        && steps.next().is_none()
}
