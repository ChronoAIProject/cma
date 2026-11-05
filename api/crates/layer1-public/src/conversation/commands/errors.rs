use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use layer2_core::sandbox::agent_io::*;

use crate::{
    conversation::failure,
    rest::{instance_problem::InstanceProblem, resource_access_problem::ResourceProblem},
};

pub(super) fn invalid_key() -> Response {
    failure(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_idempotency_key",
        "Supply one Idempotency-Key containing 1 to 256 bytes without control characters.",
    )
}

pub(crate) fn receipt_headers(mut response: Response, receipt: &CommandReceiptView) -> Response {
    response.headers_mut().insert(
        "x-cma-command-id",
        receipt.command_id.to_string().parse().unwrap(),
    );
    if let Some(id) = &receipt.response_id {
        response
            .headers_mut()
            .insert("x-cma-response-id", id.to_string().parse().unwrap());
    }
    response
}

pub(super) fn observation_timeout() -> Response {
    failure(StatusCode::GATEWAY_TIMEOUT, "agent_command_observation_timeout",
        "The admitted command has no observed turn binding yet. Its outcome remains unknown; retry the same input with the retained Idempotency-Key or read its command status.")
}

pub(crate) fn admission_failure(error: CommandAdmissionError) -> Response {
    match error {
        CommandAdmissionError::Access(error) => ResourceProblem(error).into_response(),
        CommandAdmissionError::Input(error) => InstanceProblem::from(error).into_response(),
        CommandAdmissionError::Authority(CommandAuthorizationError::Revoked) => InstanceProblem::Forbidden.into_response(),
        CommandAdmissionError::Ledger(CommandLedgerError::IdempotencyConflict) => failure(
            StatusCode::CONFLICT, "idempotency_key_reused", "This Idempotency-Key already identifies different command input."),
        CommandAdmissionError::Ledger(CommandLedgerError::NotFound) => InstanceProblem::AgentNotFound.into_response(),
        CommandAdmissionError::Ledger(CommandLedgerError::NotAdmissible) => InstanceProblem::AgentNotReady.into_response(),
        CommandAdmissionError::Ledger(CommandLedgerError::QueueFull) => InstanceProblem::MessageQueueFull { limit: 64 }.into_response(),
        CommandAdmissionError::Ledger(CommandLedgerError::Invalid) | CommandAdmissionError::ModelMismatch => InstanceProblem::InvalidRequest.into_response(),
        CommandAdmissionError::Ledger(CommandLedgerError::Conflict | CommandLedgerError::StaleClaim) => InstanceProblem::OperationInFlight.into_response(),
        CommandAdmissionError::Ledger(CommandLedgerError::Corrupt | CommandLedgerError::Unavailable)
        | CommandAdmissionError::Authority(CommandAuthorizationError::Unavailable) => failure(StatusCode::SERVICE_UNAVAILABLE,
            "agent_command_unavailable", "Command admission or observation is unavailable. The outcome may be unknown; retain the same input and Idempotency-Key for recovery."),
    }
}

pub(super) fn unbound_failure(command: &AgentCommand, status: &CommandStatusView) -> Response {
    let (http, code, detail) = match status.failure {
        Some(CommandFailureCode::TargetTurnChanged) => match command {
            AgentCommand::Message {
                order: AgentInputOrder::AfterTurn { .. },
                ..
            } => return crate::responses::previous_conflict(),
            AgentCommand::AnswerApproval { approval_id, .. } => {
                return InstanceProblem::ApprovalNotPending {
                    approval_id: approval_id.clone(),
                }
                .into_response()
            }
            _ => (
                StatusCode::CONFLICT,
                "target_turn_changed",
                "The command's target turn is no longer current.",
            ),
        },
        Some(CommandFailureCode::QueueTimeout) => (
            StatusCode::GATEWAY_TIMEOUT,
            "agent_command_queue_timeout",
            "The command's queue deadline expired before dispatch.",
        ),
        Some(CommandFailureCode::AuthorityRevoked) => (
            StatusCode::FORBIDDEN,
            "agent_command_authority_revoked",
            "The command was refused because execution authority was revoked.",
        ),
        Some(CommandFailureCode::AuthorityUnavailable) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "agent_command_authority_unavailable",
            "The command was refused because execution authority could not be verified.",
        ),
        Some(CommandFailureCode::CredentialSetupRequired) => (
            StatusCode::CONFLICT,
            "sandbox_credential_setup_required",
            "The Sandbox credential must be set up again before the Agent can resume. Ask its owner to restore credentials and submit a new command.",
        ),
        Some(CommandFailureCode::ConnectionsRequired) => (
            StatusCode::CONFLICT,
            "connections_required",
            "The Sandbox owner must reconnect its required services before the Agent can resume. Restore the connections and submit a new command.",
        ),
        Some(CommandFailureCode::AgentUnavailable) => (
            StatusCode::CONFLICT,
            "agent_unavailable",
            "The Agent became unavailable before this command could dispatch.",
        ),
        Some(CommandFailureCode::RuntimeUnavailable) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "runtime_unavailable",
            "The command was refused before dispatch because its runtime was unavailable.",
        ),
        Some(CommandFailureCode::RuntimeLost) => (
            StatusCode::CONFLICT,
            "runtime_lost",
            "The runtime was lost before this command could dispatch.",
        ),
        Some(CommandFailureCode::ExecutionFailed) => (
            StatusCode::CONFLICT,
            "agent_command_execution_failed",
            "The command failed before a turn was bound.",
        ),
        None if status.status == AgentResponseStatus::Cancelled => (
            StatusCode::CONFLICT,
            "agent_command_cancelled",
            "This queued command was cancelled before dispatch.",
        ),
        None => return InstanceProblem::Unavailable.into_response(),
    };
    failure(http, code, detail)
}
