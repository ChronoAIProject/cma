use std::collections::BTreeMap;

use layer3_adapter::AgentProcessAction::{Close, Open};
use layer3_adapter::{AgentCloseRequest, AgentSlot};

use super::{Operation, Record, Status, Subject};
use crate::failure::{Result, RuntimeError};

pub(super) fn validate_records(records: &[Record]) -> Result<()> {
    let numbered = records
        .iter()
        .filter(|record| uses_incarnation(&record.subject, &record.operation_id));
    let mut numbered = numbered.peekable();
    if numbered.peek().is_none() {
        return Ok(());
    }
    let by_id: BTreeMap<_, _> = records
        .iter()
        .map(|record| (record.operation_id.as_str(), record))
        .collect();
    for record in numbered {
        validate(&record.operation(), |id| {
            Ok(by_id.get(id).copied().cloned())
        })?;
    }
    Ok(())
}

pub(crate) fn uses_incarnation(subject: &Subject, operation_id: &str) -> bool {
    match subject {
        Subject::OpenAgent { .. } => !operation_id.ends_with(":open:v1"),
        Subject::CloseAgent { .. } => !operation_id.ends_with(":close:v1"),
        _ => false,
    }
}

pub(crate) fn validate(
    operation: &Operation,
    mut load: impl FnMut(&str) -> Result<Option<Record>>,
) -> Result<()> {
    match &operation.subject {
        Subject::OpenAgent { agent_id, .. } => {
            let incarnation = Open
                .incarnation(agent_id, &operation.operation_id)
                .ok_or_else(invalid)?;
            if incarnation == 1 {
                return Ok(());
            }
            let previous_open = Open.operation_id(agent_id, incarnation - 1);
            let previous_close = Operation::close_agent(&AgentCloseRequest::new(
                Close.operation_id(agent_id, incarnation - 1),
                AgentSlot::new(agent_id),
            ));
            let opened = load(&previous_open)?.ok_or_else(invalid)?;
            let closed = load(&previous_close.operation_id)?.ok_or_else(invalid)?;
            if opened.status != Status::Completed
                || opened.operation_id != previous_open
                || opened.subject != operation.subject
                || closed.status != Status::Completed
            {
                return Err(invalid());
            }
            closed.exact(&previous_close)?;
        }
        Subject::CloseAgent { agent_id } => {
            let incarnation = Close
                .incarnation(agent_id, &operation.operation_id)
                .ok_or_else(invalid)?;
            if incarnation == 1 {
                return Ok(());
            }
            let open_id = Open.operation_id(agent_id, incarnation);
            let opened = load(&open_id)?.ok_or_else(invalid)?;
            if opened.operation_id != open_id
                || !matches!(&opened.subject, Subject::OpenAgent { agent_id: opened_id, .. } if opened_id == agent_id)
            {
                return Err(invalid());
            }
        }
        _ => {}
    }
    Ok(())
}

fn invalid() -> RuntimeError {
    RuntimeError::rejected(
        "agent_process_history_invalid",
        "Agent process history is missing its exact predecessor or names a different home or worktree",
    )
}
