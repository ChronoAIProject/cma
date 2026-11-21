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
    }
}

pub(super) fn admission(value: CommandAdmissionError) -> Response {
    match value {
        CommandAdmissionError::Ledger(CommandLedgerError::Conflict | CommandLedgerError::NotAdmissible | CommandLedgerError::StaleClaim) => error::error(StatusCode::CONFLICT,
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
