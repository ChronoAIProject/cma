use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::resource_access::{
    AccessError, MembershipError, ResourceAccessError, ResourceAccessStoreError,
};
use layer2_core::sandbox::{
    agent_io::{CommandAdmissionError, CommandAuthorizationError, CommandLedgerError},
    AgentReadError,
};
use serde_json::json;

pub(super) fn error(
    status: StatusCode,
    code: &str,
    message: &str,
    param: Option<&str>,
) -> Response {
    let mut response = (
        status,
        Json(json!({"error": {
            "type": if status.is_server_error() { "server_error" } else { "invalid_request_error" },
            "code": code, "message": message, "param": param,
        }})),
    )
        .into_response();
    if matches!(
        status,
        StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE
    ) {
        response
            .headers_mut()
            .insert("retry-after", "1".parse().unwrap());
    }
    response
}
pub(super) fn field(field: &str, code: &str, message: &str) -> Response {
    error(StatusCode::UNPROCESSABLE_ENTITY, code, message, Some(field))
}
pub fn missing() -> Response {
    error(
        StatusCode::NOT_FOUND,
        "response_not_found",
        "The requested response or conversation is unavailable.",
        None,
    )
}
pub fn unauthenticated() -> Response {
    error(
        StatusCode::UNAUTHORIZED,
        "authentication_required",
        "A current authenticated request credential is required.",
        None,
    )
}
pub(super) fn unavailable() -> Response {
    error(StatusCode::SERVICE_UNAVAILABLE, "response_unavailable", "The response cannot be observed right now. Retry with the same idempotency key or retrieve the admitted response.", None)
}

pub(super) fn admission_paused() -> Response {
    let mut response = error(
        StatusCode::SERVICE_UNAVAILABLE,
        "admission_paused",
        layer2_core::admission::ADMISSION_PAUSED_GUIDANCE,
        None,
    );
    response
        .headers_mut()
        .insert("retry-after", "60".parse().unwrap());
    response
}

pub(super) fn session_capacity() -> Response {
    let mut response = error(
        StatusCode::SERVICE_UNAVAILABLE,
        "session_capacity_unavailable",
        layer2_core::admission::CapacityKind::ActiveSessions.guidance(),
        None,
    );
    response
        .headers_mut()
        .insert("retry-after", "60".parse().unwrap());
    response
}

pub(super) fn home_capacity() -> Response {
    let mut response = error(
        StatusCode::SERVICE_UNAVAILABLE,
        "sandbox_capacity_unavailable",
        layer2_core::admission::CapacityKind::RetainedHomes.guidance(),
        None,
    );
    response
        .headers_mut()
        .insert("retry-after", "60".parse().unwrap());
    response
}

pub(super) fn binding_admission(value: CommandAdmissionError) -> Response {
    if matches!(
        value,
        CommandAdmissionError::Ledger(CommandLedgerError::NotFound)
            | CommandAdmissionError::Access(ResourceAccessError::Access(AccessError::NotFound))
            | CommandAdmissionError::Access(ResourceAccessError::Store(
                ResourceAccessStoreError::NotFound
            ))
    ) {
        return error(
            StatusCode::GONE,
            "partition_agent_missing",
            "The binding conversation's Agent is missing. Inspect the binding conversation mapping before reconnecting the channel.",
            Some("conversation"),
        );
    }
    if matches!(
        value,
        CommandAdmissionError::Ledger(CommandLedgerError::NotAdmissible)
    ) {
        return error(
            StatusCode::CONFLICT,
            "partition_agent_unavailable",
            "The binding conversation's Agent cannot accept input. Inspect its lifecycle state before retrying.",
            Some("conversation"),
        );
    }
    // A concurrent Agent/lifecycle write invalidates the reservation snapshot.
    // Binding clients retry 429 with the same event and key under their bounded
    // provider retry policy; permanent input and admissibility refusals remain
    // distinct and must not be reclassified as capacity.
    if matches!(
        value,
        CommandAdmissionError::Ledger(CommandLedgerError::Conflict)
    ) {
        error(
            StatusCode::TOO_MANY_REQUESTS,
            "agent_command_admission_conflict",
            "The Agent changed while admitting this event. Retry the same request and idempotency key.",
            None,
        )
    } else {
        admission(value)
    }
}
pub(super) fn read(error: AgentReadError) -> Response {
    tracing::debug!(error = %error, "Responses v2 history read refused");
    match error {
        AgentReadError::NotFound => missing(),
        _ => unavailable(),
    }
}
pub(super) fn admission(value: CommandAdmissionError) -> Response {
    tracing::debug!(error = %value, "Responses v2 command request refused");
    if let CommandAdmissionError::Ledger(error) = &value {
        // Stable categories distinguish stale observations from closed admission
        // at production log levels without recording input, keys or credentials.
        tracing::info!(
            reason = error.code(),
            "Responses v2 command admission refused"
        );
    }
    match value {
        CommandAdmissionError::Ledger(CommandLedgerError::SessionCapacityUnavailable)
        | CommandAdmissionError::Input(
            layer2_core::sandbox::AgentOperationError::SessionCapacityUnavailable,
        ) => session_capacity(),
        CommandAdmissionError::Ledger(CommandLedgerError::AdmissionPaused)
        | CommandAdmissionError::Input(
            layer2_core::sandbox::AgentOperationError::AdmissionPaused,
        ) => admission_paused(),
        CommandAdmissionError::Input(
            layer2_core::sandbox::AgentOperationError::ModelFundingUnsupported,
        ) => model_funding(layer2_core::sandbox::model_credit::FundingError::Unsupported),
        CommandAdmissionError::Input(layer2_core::sandbox::AgentOperationError::ModelFunding(
            reason,
        )) => model_funding(reason),
        CommandAdmissionError::ModelMismatch => field(
            "model",
            "model_mismatch",
            "model must match the current Agent conversation settings.",
        ),
        CommandAdmissionError::Input(_) => field(
            "input",
            "invalid_response_input",
            "The Agent accepts bounded text input; this input is invalid or unsupported.",
        ),
        CommandAdmissionError::Authority(CommandAuthorizationError::Revoked) => error(
            StatusCode::FORBIDDEN,
            "command_authority_required",
            "Current credential custody does not authorize durable command execution.",
            None,
        ),
        CommandAdmissionError::Ledger(CommandLedgerError::NotFound)
        | CommandAdmissionError::Access(ResourceAccessError::Access(AccessError::NotFound))
        | CommandAdmissionError::Access(ResourceAccessError::Store(
            ResourceAccessStoreError::NotFound,
        ))
        | CommandAdmissionError::Access(ResourceAccessError::Membership(
            MembershipError::NotMember,
        )) => missing(),
        CommandAdmissionError::Access(ResourceAccessError::Access(
            AccessError::Forbidden | AccessError::UnsafeInteractiveShare,
        )) => forbidden(),
        CommandAdmissionError::Ledger(CommandLedgerError::Invalid) => field(
            "Idempotency-Key",
            "invalid_idempotency_key",
            "Idempotency-Key must contain 1 to 256 bytes without control characters.",
        ),
        CommandAdmissionError::Ledger(CommandLedgerError::QueueFull) => error(
            StatusCode::TOO_MANY_REQUESTS,
            "agent_command_queue_full",
            "The Agent command queue is full. Retry the same request after capacity is available.",
            None,
        ),
        // A final 402: the helper adds Retry-After only to 429 and 503.
        CommandAdmissionError::Ledger(CommandLedgerError::Budget(reason)) => error(
            StatusCode::PAYMENT_REQUIRED,
            reason.code(),
            reason.guidance(),
            None,
        ),
        CommandAdmissionError::Ledger(CommandLedgerError::IdempotencyConflict) => error(
            StatusCode::CONFLICT,
            "agent_command_key_conflict",
            "This idempotency key already identifies different input.",
            Some("Idempotency-Key"),
        ),
        CommandAdmissionError::Ledger(CommandLedgerError::NotAdmissible) => error(
            StatusCode::CONFLICT,
            "agent_command_not_admissible",
            "This request was not admitted. Its answer or Agent is no longer eligible.",
            None,
        ),
        CommandAdmissionError::Ledger(
            CommandLedgerError::Conflict | CommandLedgerError::StaleClaim,
        ) => error(
            StatusCode::CONFLICT,
            "agent_command_admission_conflict",
            "The Agent cannot admit the request from this observation. Retry using the same key.",
            None,
        ),
        _ => unavailable(),
    }
}

fn forbidden() -> Response {
    error(
        StatusCode::FORBIDDEN,
        "response_forbidden",
        "This resource does not permit the requested interaction.",
        None,
    )
}

fn model_funding(reason: layer2_core::sandbox::model_credit::FundingError) -> Response {
    if reason == layer2_core::sandbox::model_credit::FundingError::Forbidden {
        return forbidden();
    }
    let (status, _, message, code) = crate::model_funding_problem::projection(reason);
    let mut response = error(status, code, message, None);
    // A funding outcome cannot authorize resending an uncertain original intent.
    response.headers_mut().remove("retry-after");
    response
}

#[cfg(test)]
mod admission_pause_tests {
    use super::*;

    #[tokio::test]
    async fn response_admission_and_binding_admission_keep_operator_pause_typed() {
        for response in [
            admission(CommandAdmissionError::Ledger(
                CommandLedgerError::AdmissionPaused,
            )),
            binding_admission(CommandAdmissionError::Ledger(
                CommandLedgerError::AdmissionPaused,
            )),
        ] {
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(response.headers()["retry-after"], "60");
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["error"]["code"], "admission_paused");
            assert_eq!(
                body["error"]["message"],
                layer2_core::admission::ADMISSION_PAUSED_GUIDANCE
            );
        }
    }
}

#[cfg(test)]
mod session_capacity_tests {
    use super::*;

    #[tokio::test]
    async fn session_capacity_refusals_preserve_the_response_error_contract() {
        for response in [
            admission(CommandAdmissionError::Ledger(
                CommandLedgerError::SessionCapacityUnavailable,
            )),
            binding_admission(CommandAdmissionError::Input(
                layer2_core::sandbox::AgentOperationError::SessionCapacityUnavailable,
            )),
        ] {
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(response.headers()["retry-after"], "60");
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["error"]["code"], "session_capacity_unavailable");
            assert_eq!(
                body["error"]["message"],
                layer2_core::admission::CapacityKind::ActiveSessions.guidance()
            );
            assert!(!body.to_string().contains("limit"));
            assert!(!body.to_string().contains("count"));
        }
    }
}

#[cfg(test)]
#[tokio::test]
async fn responses_model_funding_contract() {
    crate::model_funding_problem_tests::assert_funding_contract(
        |value| admission(value.into()),
        true,
    )
    .await;
}

#[cfg(test)]
#[tokio::test]
async fn responses_invalid_input_remains_invalid() {
    crate::model_funding_problem_tests::assert_invalid_contract(
        |value| admission(value.into()),
        "invalid_response_input",
    )
    .await;
}

#[cfg(test)]
#[tokio::test]
async fn binding_model_funding_contract() {
    crate::model_funding_problem_tests::assert_funding_contract(
        |value| binding_admission(value.into()),
        true,
    )
    .await;
}

#[cfg(test)]
#[tokio::test]
async fn binding_invalid_input_remains_invalid() {
    crate::model_funding_problem_tests::assert_invalid_contract(
        |value| binding_admission(value.into()),
        "invalid_response_input",
    )
    .await;
}
