use crate::providers::ProviderError;
use cmaeg_core::problem::ProblemCode;

/// The attempted step, including its local preparation and admission checks.
#[derive(Clone, Copy)]
pub(super) enum ProviderCall {
    Responses,
    Conversation,
}

pub(super) struct StartFailure {
    pub error: ProviderError,
    call: ProviderCall,
}

impl ProviderCall {
    pub fn failed(self, error: ProviderError) -> StartFailure {
        StartFailure { error, call: self }
    }
}

/// Classify a start failure and emit its single warn record using bounded codes
/// and static labels, never a provider error's Display/Debug.
pub(super) fn classify_start_failure(
    provider: &str,
    failure: StartFailure,
) -> (ProblemCode, &'static str, u16) {
    let classification = classify(&failure.error);
    let provider_call = match failure.call {
        ProviderCall::Responses => "responses",
        ProviderCall::Conversation => "conversation",
    };
    let (provider_error, upstream_status, upstream_code) = match &failure.error {
        ProviderError::Rejected { status, code } => {
            ("rejected", Some(*status), Some(code_token(code)))
        }
        ProviderError::UnknownProvider(_) => ("unknown_provider", None, None),
        ProviderError::InvalidRequest(_) => ("invalid_request", None, None),
        ProviderError::Busy { .. } => ("busy", None, None),
        ProviderError::Capacity => ("capacity", None, None),
        ProviderError::ConversationMissing => ("conversation_missing", None, None),
        ProviderError::Unavailable(_) => ("unavailable", None, None),
        ProviderError::ResponseMalformed(_) => ("response_malformed", None, None),
        ProviderError::Sse(_) => ("sse", None, None),
    };
    tracing::warn!(
        provider,
        provider_call,
        gateway_detail = classification.1,
        gateway_status = classification.2,
        provider_error,
        upstream_status,
        upstream_code,
        "provider turn start failed"
    );
    classification
}

fn code_token(code: &str) -> &str {
    if (1..=64).contains(&code.len())
        && code.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-.:".contains(&byte)
        })
    {
        code
    } else {
        "unrecognized"
    }
}

fn classify(error: &ProviderError) -> (ProblemCode, &'static str, u16) {
    match error {
        ProviderError::Capacity | ProviderError::Busy { .. } => {
            (ProblemCode::ProviderBusy, "provider_busy", 503)
        }
        ProviderError::Rejected { status, code } => {
            let detail = match (*status, code.as_str()) {
                (410, "partition_agent_missing") => "partition_agent_missing",
                (409, "partition_agent_unavailable") => "partition_agent_unavailable",
                (409, "agent_command_admission_conflict") => "agent_command_admission_conflict",
                (409, "agent_command_key_conflict") => "agent_command_key_conflict",
                (409, "binding_not_ready") => "binding_not_ready",
                (409, "binding_resources_unavailable") => "binding_resources_unavailable",
                (403, "command_authority_required") => "command_authority_required",
                (403, "response_forbidden") => "response_forbidden",
                (404, "response_not_found") => "response_not_found",
                (422, "model_mismatch") => "model_mismatch",
                _ => return (ProblemCode::ProviderRejected, "provider_rejected", 502),
            };
            (ProblemCode::ProviderRejected, detail, *status)
        }
        _ => (
            ProblemCode::ProviderUnavailable,
            "provider_unavailable",
            503,
        ),
    }
}
