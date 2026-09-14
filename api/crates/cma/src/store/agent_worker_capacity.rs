//! Evidence-gated access to the agent log's terminal reserves.

use layer2_core::sandbox::store::AgentWorkMutation;
use layer2_core::sandbox::{Agent, AgentLogKind, AgentStatus, SandboxEventKind};

use super::agent_log_integrity::AppendCapacity;

pub(super) fn publication_capacity(
    current: &Agent,
    mutation: &AgentWorkMutation,
) -> AppendCapacity {
    if delete_evidence_matches(current, mutation) {
        AppendCapacity::Delete
    } else if stop_evidence_matches(current, mutation)
        || startup_failure_evidence_matches(current, mutation)
    {
        AppendCapacity::StopOrFailure
    } else {
        AppendCapacity::Normal
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
