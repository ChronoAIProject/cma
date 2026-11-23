//! Runtime-owned Agent history export boundary.

use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentHistoryExportError {
    Unavailable,
    Capacity,
    Unsupported,
    Rejected,
}

pub(super) fn problem(error: AgentHistoryExportError) -> axum::response::Response {
    use axum::http::StatusCode;
    let (code, detail) = match error {
        AgentHistoryExportError::Capacity => return super::problems::runtime_capacity(
            layer2_core::sandbox::RuntimeCapacityError::Request,
        ),
        AgentHistoryExportError::Unavailable => (
            "history_export_unavailable",
            "Raw history is currently unavailable from the runtime. Retry later or export the retained conversation.",
        ),
        AgentHistoryExportError::Unsupported => (
            "history_export_unsupported",
            "This runtime does not support raw history export. The retained conversation can be exported separately.",
        ),
        AgentHistoryExportError::Rejected => (
            "history_export_rejected",
            "The runtime could not provide a valid raw history export. The retained conversation can be exported separately.",
        ),
    };
    let status = StatusCode::SERVICE_UNAVAILABLE;
    super::problem_response(
        status,
        super::problem_document(status, "agent-history-export", detail, code),
    )
}

#[async_trait]
pub trait AgentHistoryExporter: Send + Sync {
    async fn export(
        &self,
        runtime_id: &str,
        agent_id: &str,
    ) -> Result<Vec<u8>, AgentHistoryExportError>;
}
