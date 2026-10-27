use axum::{http::HeaderMap, response::Response};
use layer2_core::{
    ids::AgentResponseId,
    sandbox::agent_io::{
        command_key_digest, AgentCommand, AgentInput, TargetedControlSubmission, TargetedResponse,
    },
    AgentId,
};

use super::super::error;
use super::shapes::{V2ResponseControlCommand, V2ResponseControlRequest};

pub(in crate::responses::v2) fn parse(
    agent_id: String,
    response_id: String,
    headers: &HeaderMap,
    request: V2ResponseControlRequest,
) -> Result<TargetedControlSubmission, Box<Response>> {
    if headers.get_all("idempotency-key").iter().count() != 1 {
        return Err(Box::new(error::field(
            "Idempotency-Key",
            "invalid_idempotency_key",
            "Supply exactly one retained idempotency key.",
        )));
    }
    let key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .filter(|key| command_key_digest(key).is_ok())
        .ok_or_else(|| {
            Box::new(error::field(
                "Idempotency-Key",
                "invalid_idempotency_key",
                "Idempotency-Key must contain 1 to 256 bytes without control characters.",
            ))
        })?;
    for (field, value) in [
        ("turn_sequence", request.turn_sequence),
        ("expected_agent_revision", request.expected_agent_revision),
    ] {
        if !(1..=i64::MAX as u64).contains(&value) {
            return Err(Box::new(error::field(
                field,
                "invalid_control_observation",
                "Use a positive signed-64-bit value from the controls observation.",
            )));
        }
    }
    for (field, value) in [("agent_id", &agent_id), ("response_id", &response_id)] {
        if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
            return Err(Box::new(error::field(
                field,
                "invalid_control_target",
                "The target identifier is invalid.",
            )));
        }
    }
    let command = match request.command {
        V2ResponseControlCommand::Cancel {} => AgentCommand::Interrupt,
        V2ResponseControlCommand::Steer { text } => AgentCommand::Steer {
            input: AgentInput::text(text),
        },
        V2ResponseControlCommand::ResolveApproval {
            approval_id,
            decision,
        } => AgentCommand::AnswerApproval {
            approval_id,
            decision: decision.into(),
        },
    }
    .canonical()
    .map_err(|_| {
        Box::new(error::field(
            "command",
            "invalid_control_request",
            "Control text or approval identity is invalid.",
        ))
    })?;
    Ok(TargetedControlSubmission {
        agent_id: AgentId(agent_id),
        target: TargetedResponse {
            response_id: AgentResponseId(response_id),
            turn_sequence: request.turn_sequence,
        },
        expected_agent_revision: request.expected_agent_revision,
        command,
        idempotency_key: key.to_owned(),
    })
}
