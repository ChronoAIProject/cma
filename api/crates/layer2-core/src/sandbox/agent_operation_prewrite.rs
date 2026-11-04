use super::*;
pub(super) fn append_pre_write_rollback(
    current: &Agent,
    next: &mut Agent,
    prepared: &PreparedAgentOperation,
    at: &str,
    logs: &mut Vec<AgentLogKind>,
    deferred: Option<String>,
) -> Result<(), AgentOperationError> {
    match &prepared.rollback {
        PreWriteRollback::Settings(previous) => {
            next.conversation.settings = previous.clone();
        }
        PreWriteRollback::NewTurn
            if current.status == AgentStatus::Working
                && current.conversation.active_turn == Some(prepared.turn_sequence) =>
        {
            next.conversation.active_turn = None;
            next.transition(AgentStatus::Ready, at.to_owned(), None)
                .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;
            logs.push(AgentLogKind::TurnFailed {
                turn_sequence: prepared.turn_sequence,
                failure: TypedFailure {
                    class: if deferred.is_some() {
                        super::super::agent_io::SOURCE_DEFERRED_FAILURE.into()
                    } else {
                        "input_not_written".into()
                    },
                    detail: deferred
                        .unwrap_or_else(|| "the message did not reach the runtime writer".into()),
                },
            });
            logs.push(AgentLogKind::StatusChanged {
                from: AgentStatus::Working,
                to: AgentStatus::Ready,
            });
        }
        PreWriteRollback::Approval(snapshot)
            if matches!(
                current.status,
                AgentStatus::Working | AgentStatus::WaitingApproval
            ) && current.conversation.active_turn == Some(prepared.turn_sequence) =>
        {
            logs.push(AgentLogKind::ApprovalRequested {
                approval_id: snapshot.approval_id.clone(),
                action: snapshot.action.clone(),
                detail: snapshot.detail.clone(),
                runtime_locator: snapshot.runtime_locator.clone(),
            });
            if current.status == AgentStatus::Working {
                next.transition(AgentStatus::WaitingApproval, at.to_owned(), None)
                    .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;
                logs.push(AgentLogKind::StatusChanged {
                    from: AgentStatus::Working,
                    to: AgentStatus::WaitingApproval,
                });
            }
        }
        PreWriteRollback::WaitingApproval
            if current.status == AgentStatus::Working
                && current.conversation.active_turn == Some(prepared.turn_sequence) =>
        {
            next.transition(AgentStatus::WaitingApproval, at.to_owned(), None)
                .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;
            logs.push(AgentLogKind::StatusChanged {
                from: AgentStatus::Working,
                to: AgentStatus::WaitingApproval,
            });
        }
        _ => {}
    }
    Ok(())
}
