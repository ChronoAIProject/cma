use axum::{http::StatusCode, response::Response};
use layer2_core::sandbox::agent_io::{
    CommandAdmissionError, CommandLedgerError, QueuedCancellationError,
};

use super::super::error;

pub(super) fn edit(value: QueuedCancellationError) -> Response {
    match value {
        QueuedCancellationError::NotAdmitted => error::error(StatusCode::CONFLICT,
            "queued_input_not_admitted",
            "The original edit was not admitted and its source command revision is permanently stale. Observe again before creating a new intent.", None),
        QueuedCancellationError::Admission(value) => admission(value),
        QueuedCancellationError::OwnerRequired => admission(CommandLedgerError::Unavailable.into()),
    }
}

pub(super) fn admission(value: CommandAdmissionError) -> Response {
    match value {
        value @ CommandAdmissionError::Input(
            layer2_core::sandbox::AgentOperationError::ModelFundingUnsupported
            | layer2_core::sandbox::AgentOperationError::ModelFunding(_),
        ) => error::admission(value),
        CommandAdmissionError::Ledger(CommandLedgerError::Conflict | CommandLedgerError::KnownAborted | CommandLedgerError::NotAdmissible | CommandLedgerError::StaleClaim) => error::error(StatusCode::CONFLICT,
            "queued_input_conflict",
            "The source command or current authority changed. Recover an uncertain edit with the original key and body.", None),
        CommandAdmissionError::Ledger(CommandLedgerError::Invalid) | CommandAdmissionError::Input(_) => invalid(),
        other => error::admission(other),
    }
}

pub(super) fn invalid() -> Response {
    error::field(
        "body",
        "invalid_queued_input_request",
        "Supply only the original source_command_id and positive expected_command_revision, and nonempty text.",
    )
}

#[cfg(test)]
#[tokio::test]
async fn queued_edit_model_funding_contract() {
    crate::model_funding_problem_tests::assert_funding_contract(
        |value| edit(QueuedCancellationError::Admission(value.into())),
        true,
    )
    .await;
}

#[cfg(test)]
#[tokio::test]
async fn queued_edit_invalid_input_remains_invalid() {
    crate::model_funding_problem_tests::assert_invalid_contract(
        |value| edit(QueuedCancellationError::Admission(value.into())),
        "invalid_queued_input_request",
    )
    .await;
}
