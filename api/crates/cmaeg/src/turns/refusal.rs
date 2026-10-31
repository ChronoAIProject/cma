use crate::providers::ProviderError;
use cmaeg_core::problem::ProblemCode;

pub(super) fn classify(error: ProviderError) -> (ProblemCode, &'static str, u16) {
    match error {
        ProviderError::Capacity | ProviderError::Busy { .. } => {
            (ProblemCode::ProviderBusy, "provider_busy", 503)
        }
        ProviderError::Rejected { status, code } => {
            let detail = match (status, code.as_str()) {
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
            (ProblemCode::ProviderRejected, detail, status)
        }
        _ => (
            ProblemCode::ProviderUnavailable,
            "provider_unavailable",
            503,
        ),
    }
}
