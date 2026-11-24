use axum::{http::StatusCode, response::Response};
use layer2_core::sandbox::agent_io::{
    CommandAdmissionError, CommandLedgerError, QueuedCancellationError,
};

use super::super::error;

pub(super) fn cancellation(value: QueuedCancellationError) -> Response {
    match value {
        QueuedCancellationError::NotAdmitted => error::error(StatusCode::CONFLICT,
            "queued_response_cancellation_not_admitted",
            "The original cancellation was not admitted and its source command revision is permanently stale. Observe again before creating a new intent.", None),
        QueuedCancellationError::Admission(value) => admission(value),
        QueuedCancellationError::OwnerRequired => {
            let mut response = error::error(
                StatusCode::SERVICE_UNAVAILABLE,
                "agent_owner_unavailable",
                "The Agent owner is temporarily unavailable. Retry with the original cancellation key and body.",
                None,
            );
            response.extensions_mut().insert(super::QueuedCancellationNeedsOwner);
            response
        }
    }
}

pub(super) fn admission(value: CommandAdmissionError) -> Response {
    match value {
        value @ CommandAdmissionError::Input(
            layer2_core::sandbox::AgentOperationError::ModelFundingUnsupported
            | layer2_core::sandbox::AgentOperationError::ModelFunding(_),
        ) => error::admission(value),
        CommandAdmissionError::Ledger(CommandLedgerError::Conflict | CommandLedgerError::NotAdmissible | CommandLedgerError::StaleClaim) => error::error(StatusCode::CONFLICT,
            "queued_response_cancellation_conflict",
            "The source command or current authority changed. Recover an uncertain cancellation with the original key and body.", None),
        CommandAdmissionError::Ledger(CommandLedgerError::Invalid) | CommandAdmissionError::Input(_) => invalid(),
        other => error::admission(other),
    }
}

pub(super) fn invalid() -> Response {
    error::field(
        "body",
        "invalid_queued_cancellation_request",
        "Supply only the original source_command_id and positive expected_command_revision.",
    )
}

#[cfg(test)]
#[tokio::test]
async fn queued_cancellation_model_funding_contract() {
    crate::model_funding_problem_tests::assert_funding_contract(
        |value| cancellation(QueuedCancellationError::Admission(value.into())),
        true,
    )
    .await;
}

#[cfg(test)]
#[tokio::test]
async fn queued_cancellation_invalid_input_remains_invalid() {
    crate::model_funding_problem_tests::assert_invalid_contract(
        |value| cancellation(QueuedCancellationError::Admission(value.into())),
        "invalid_queued_cancellation_request",
    )
    .await;
}
