//! Adopt only output progress of the original turn when releasing a reconnect claim.
use super::connection_retry::same_connection_claim;
use layer2_core::sandbox::{Agent, AgentStatus, FirstMessageDelivery};
use std::time::SystemTime;

pub(in crate::sandbox) fn same_recovery_claim(
    claimed: &Agent,
    current: &Agent,
    holder: &str,
    now: SystemTime,
) -> bool {
    let Some(turn) = claimed.conversation.active_turn else {
        return false;
    };
    if !matches!(
        claimed.status,
        AgentStatus::Working | AgentStatus::WaitingApproval
    ) || !matches!(
        (current.status, current.conversation.active_turn),
        (AgentStatus::Ready, None) | (AgentStatus::Working | AgentStatus::WaitingApproval, Some(_))
    ) || current
        .conversation
        .active_turn
        .is_some_and(|active| active != turn)
    {
        return false;
    }
    if claimed
        .conversation
        .observations
        .task_acceptance
        .as_ref()
        .map(|task| (&task.task_id, task.turn_sequence))
        != current
            .conversation
            .observations
            .task_acceptance
            .as_ref()
            .map(|task| (&task.task_id, task.turn_sequence))
    {
        return false;
    }
    let mut normalized = current.clone();
    if claimed.ready_at.is_none()
        && current.status == AgentStatus::Ready
        && current.conversation.active_turn.is_none()
    {
        normalized.ready_at = None;
    }
    normalized.status = claimed.status;
    normalized.conversation.active_turn = Some(turn);
    normalized.conversation.observations = claimed.conversation.observations.clone();
    if let (Some(before), Some(after)) = (
        &claimed.conversation.first_message,
        &mut normalized.conversation.first_message,
    ) {
        let mut expected = before.clone();
        if after.execution_observed_at != before.execution_observed_at {
            let Some(at) = &after.execution_observed_at else {
                return false;
            };
            if expected.mark_execution_observed(at.clone()).is_err() {
                return false;
            }
        }
        if after.state != expected.state {
            let result = match after.state {
                FirstMessageDelivery::Completed => match &after.completed_at {
                    Some(at) => expected.complete(at.clone()),
                    None => return false,
                },
                FirstMessageDelivery::Failed => match &after.failure {
                    Some(failure) => expected.fail(failure.clone()),
                    None => return false,
                },
                _ => return false,
            };
            if result.is_err() {
                return false;
            }
        }
        if expected != *after {
            return false;
        }
        *after = before.clone();
    }
    if let (Some(before), Some(after)) = (
        &claimed.conversation.operation,
        &mut normalized.conversation.operation,
    ) {
        if before != after {
            let Some(at) = &after.execution_observed_at else {
                return false;
            };
            let mut expected = before.clone();
            if expected.mark_execution_observed(at.clone()).is_err() || expected != *after {
                return false;
            }
        }
        *after = before.clone();
    }
    for (before, after) in claimed
        .conversation
        .queued_messages
        .iter()
        .zip(&mut normalized.conversation.queued_messages)
    {
        if before != after {
            let mut expected = before.clone();
            if expected.mark_execution_observed().is_err() || expected != *after {
                return false;
            }
        }
        *after = before.clone();
    }
    same_connection_claim(claimed, &normalized, holder, now)
}
