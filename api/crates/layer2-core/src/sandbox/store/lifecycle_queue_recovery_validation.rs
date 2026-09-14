use crate::sandbox::queued_message::{
    QUEUED_MESSAGE_DISPATCH_GRACE_SECONDS, QUEUED_MESSAGE_RECOVERY_CODE,
    QUEUED_MESSAGE_RECOVERY_DETAIL,
};
use crate::sandbox::{
    Agent, AgentLogKind, AgentOperationDelivery, AgentOperationFailure, AgentOperationIntent,
    AgentStatus, QueuedMessageState,
};
use crate::seconds_from_rfc3339;

use super::{SandboxLifecycleAgentMutation, SandboxStoreError};

pub(super) fn validate_queue_recovery(
    current: &Agent,
    change: &SandboxLifecycleAgentMutation,
    at: &str,
) -> Result<(), SandboxStoreError> {
    let next = &change.agent;
    let operation = current
        .conversation
        .operation
        .as_ref()
        .ok_or_else(|| invalid("queue recovery has no operation fence"))?;
    let queue_index = current
        .conversation
        .queued_messages
        .iter()
        .position(|message| {
            message.state != QueuedMessageState::Delivered
                && message.dispatch_fenced_at.as_deref()
                    == Some(operation.dispatch_fenced_at.as_str())
                && matches!(
                    &operation.intent,
                    AgentOperationIntent::SendMessage { payload_sha256 }
                        if payload_sha256 == &message.payload_sha256
                )
        })
        .ok_or_else(|| invalid("queue recovery has no matching FIFO fence"))?;
    let queued = &current.conversation.queued_messages[queue_index];
    let observed_at =
        seconds_from_rfc3339(at).ok_or_else(|| invalid("queue recovery time is invalid"))?;
    let fenced_at = seconds_from_rfc3339(&operation.dispatch_fenced_at)
        .ok_or_else(|| invalid("queue recovery fence time is invalid"))?;
    let stale = fenced_at
        .checked_add(QUEUED_MESSAGE_DISPATCH_GRACE_SECONDS)
        .is_some_and(|deadline| observed_at >= deadline);
    if current.status != AgentStatus::Working
        || current.conversation.active_turn != Some(operation.turn_sequence)
        || operation.state != AgentOperationDelivery::DispatchFenced
        || queued.state != QueuedMessageState::DispatchFenced
        || current
            .conversation
            .queued_messages
            .iter()
            .position(|message| message.state != QueuedMessageState::Delivered)
            != Some(queue_index)
        || !stale
        || next.status != current.status
    {
        return Err(invalid(
            "queue recovery did not retire one stale FIFO fence",
        ));
    }

    let failure = AgentOperationFailure::new(
        QUEUED_MESSAGE_RECOVERY_CODE,
        QUEUED_MESSAGE_RECOVERY_DETAIL,
        at,
    )
    .map_err(|_| invalid("queue recovery failure is invalid"))?;
    let mut expected_conversation = current.conversation.clone();
    expected_conversation
        .operation
        .as_mut()
        .expect("the queue recovery operation was checked")
        .require_recovery(failure)
        .map_err(|_| invalid("queue operation cannot require recovery"))?;
    expected_conversation.queued_messages[queue_index]
        .require_recovery()
        .map_err(|_| invalid("queue item cannot require recovery"))?;
    expected_conversation.last_seq = expected_conversation
        .last_seq
        .checked_add(1)
        .ok_or(SandboxStoreError::SequenceConflict)?;
    let expected_logs = vec![AgentLogKind::OperationDeliveryChanged {
        operation_sequence: operation.sequence,
        operation: operation.intent.kind(),
        state: AgentOperationDelivery::RecoveryRequired,
    }];
    if next.conversation != expected_conversation || change.logs != expected_logs {
        return Err(invalid("queue-recovery agent mutation is not exact"));
    }
    Ok(())
}

const fn invalid(detail: &'static str) -> SandboxStoreError {
    SandboxStoreError::InvalidTransaction(detail)
}
