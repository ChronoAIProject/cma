//! Evidence-gated access to the agent log's terminal reserves.

use layer2_core::sandbox::store::{
    agent_cleanup_capacity_retry, AgentWorkMutation, AGENT_LOG_CAPACITY_FAILURE,
};
use layer2_core::sandbox::{Agent, AgentLogKind, AgentStatus, AgentWorkStep, SandboxEventKind};

use super::agent_log_integrity::AppendCapacity;

pub(super) fn publication_capacity(
    current: &Agent,
    mutation: &AgentWorkMutation,
) -> AppendCapacity {
    // The transaction proves that a cleanup marker has not already been
    // appended at this step. Subsequent retries carry no log entries.
    if agent_cleanup_capacity_retry(current, mutation) {
        return if current.status.is_live() {
            AppendCapacity::StopOrFailure
        } else {
            AppendCapacity::Delete
        };
    }
    if delete_evidence_matches(current, mutation)
        || cleanup_completion_capacity(current, mutation) == Some(AppendCapacity::Delete)
    {
        AppendCapacity::Delete
    } else if stop_evidence_matches(current, mutation)
        || startup_failure_evidence_matches(current, mutation)
        || capacity_failure_evidence_matches(current, mutation)
        || cleanup_completion_capacity(current, mutation) == Some(AppendCapacity::StopOrFailure)
    {
        AppendCapacity::StopOrFailure
    } else {
        AppendCapacity::Normal
    }
}

pub(super) fn capacity_failure_evidence_matches(
    current: &Agent,
    mutation: &AgentWorkMutation,
) -> bool {
    let next = &mutation.agent;
    current.status.is_live()
        && current.requested_stop_reason.is_none()
        && next.status == AgentStatus::Failed
        && next.work.step == AgentWorkStep::None
        && next.updated_at == mutation.appended_at
        && matches!(mutation.agent_logs.as_slice(),
            [AgentLogKind::WorkStepFailed { step, failure }]
                if *step == current.work.step
                    && failure.class == AGENT_LOG_CAPACITY_FAILURE
                    && Some(failure) == next.failure.as_ref())
        && transition_event_matches(current, mutation, AgentStatus::Failed)
}

/// Owner-requested stop is a prerequisite for deletion of a live Agent. Only
/// its validated finite completions (or single bounded retry marker) may cross
/// the stop ceiling; ordinary work and conversation settlement never may.
pub(super) fn stop_can_use_delete_reserve(current: &Agent, mutation: &AgentWorkMutation) -> bool {
    current.requested_stop_reason.is_some()
        && matches!(
            current.work.step,
            AgentWorkStep::FenceInteraction
                | AgentWorkStep::CloseProcess
                | AgentWorkStep::PublishStopped
        )
        && (stop_evidence_matches(current, mutation)
            || cleanup_completion_capacity(current, mutation)
                == Some(AppendCapacity::StopOrFailure)
            || agent_cleanup_capacity_retry(current, mutation))
}

/// The final terminal write is unreachable unless its preceding cleanup steps
/// can also complete. Only finite, forward progress gets this headroom; retry
/// records must not consume the reserve indefinitely. The transaction validates
/// the full publication, including close receipts and parent absence proofs,
/// before selecting capacity here.
fn cleanup_completion_capacity(
    current: &Agent,
    mutation: &AgentWorkMutation,
) -> Option<AppendCapacity> {
    let next = &mutation.agent;
    if current.status != next.status
        || next.updated_at != mutation.appended_at
        || next.requested_stop_reason != current.requested_stop_reason
        || !matches!(mutation.agent_logs.as_slice(),
            [AgentLogKind::WorkStepCompleted { step }] if *step == current.work.step)
    {
        return None;
    }
    match (current.work.step, next.work.step) {
        (AgentWorkStep::FenceInteraction, AgentWorkStep::CloseProcess)
        | (AgentWorkStep::CloseProcess, AgentWorkStep::PublishStopped)
            if current.status.is_live() && current.requested_stop_reason.is_some() =>
        {
            Some(AppendCapacity::StopOrFailure)
        }
        (AgentWorkStep::EnsureProcessClosed, AgentWorkStep::RemoveHome)
        | (AgentWorkStep::RemoveHome, AgentWorkStep::RemoveWorktree)
        | (AgentWorkStep::RemoveWorktree, AgentWorkStep::PublishDeleted)
            if matches!(current.status, AgentStatus::Stopped | AgentStatus::Failed) =>
        {
            Some(AppendCapacity::Delete)
        }
        _ => None,
    }
}

fn stop_evidence_matches(current: &Agent, mutation: &AgentWorkMutation) -> bool {
    let next = &mutation.agent;
    let Some(reason) = next.stop_reason.as_ref() else {
        return false;
    };
    current.status.is_live()
        && next.status == AgentStatus::Stopped
        && next.stopped_at.as_deref() == Some(&mutation.appended_at)
        && next.updated_at == mutation.appended_at
        && next.requested_stop_reason.is_none()
        && matches!(
            mutation.agent_logs.as_slice(),
            [AgentLogKind::Stopped { reason: logged }] if logged == reason.as_str()
        )
        && transition_event_matches(current, mutation, AgentStatus::Stopped)
}

fn startup_failure_evidence_matches(current: &Agent, mutation: &AgentWorkMutation) -> bool {
    let next = &mutation.agent;
    let Some(failure) = next.failure.as_ref() else {
        return false;
    };
    current.status == AgentStatus::Starting
        && next.status == AgentStatus::Failed
        && next.updated_at == mutation.appended_at
        && matches!(
            mutation.agent_logs.as_slice(),
            [AgentLogKind::WorkStepFailed { step, failure: logged }]
                if *step == current.work.step && logged == failure
        )
        && transition_event_matches(current, mutation, AgentStatus::Failed)
}

fn delete_evidence_matches(current: &Agent, mutation: &AgentWorkMutation) -> bool {
    let next = &mutation.agent;
    matches!(current.status, AgentStatus::Stopped | AgentStatus::Failed)
        && next.status == AgentStatus::Deleted
        && next.deleted_at.as_deref() == Some(&mutation.appended_at)
        && next.updated_at == mutation.appended_at
        && matches!(mutation.agent_logs.as_slice(), [AgentLogKind::Deleted])
        && match mutation.sandbox_event.as_ref() {
            None => true,
            Some(SandboxEventKind::AgentDeleted { agent_id }) => agent_id == &next.id,
            Some(_) => false,
        }
}

fn transition_event_matches(
    current: &Agent,
    mutation: &AgentWorkMutation,
    terminal: AgentStatus,
) -> bool {
    match mutation.sandbox_event.as_ref() {
        None => true,
        Some(SandboxEventKind::AgentStatusChanged { agent_id, from, to }) => {
            agent_id == &mutation.agent.id && *from == current.status && *to == terminal
        }
        Some(_) => false,
    }
}
