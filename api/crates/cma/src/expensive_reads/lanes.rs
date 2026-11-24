//! Exact read contracts select independent process-local payload budgets.
use super::{Duration, MatchedPath, Request, READ_TIMEOUT, RESPONSE_BYTES};
use axum::http::Method;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Lane {
    Bulk,
    Observation,
    Report,
}

impl Lane {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Bulk => "bulk",
            Self::Observation => "agent_observation",
            Self::Report => "report_descriptor",
        }
    }
    pub(super) fn bytes(self) -> usize {
        match self {
            Self::Bulk => RESPONSE_BYTES,
            Self::Observation => 64 * 1024,
            Self::Report => 1024 * 1024,
        }
    }
    pub(super) fn timeout(self) -> Duration {
        match self {
            Self::Report => Duration::from_secs(5),
            _ => READ_TIMEOUT,
        }
    }
    pub(super) fn retry_after(self) -> &'static str {
        match self {
            Self::Report => "2",
            _ => "1",
        }
    }
}

pub(super) fn classify(request: &Request) -> Option<Lane> {
    if !matches!(request.method(), &Method::GET | &Method::HEAD) {
        return None;
    }
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or_else(|| request.uri().path());
    // Usage has a fixed small response but its historical fallback still reads
    // the log. Keep admission bounded without consuming transcript capacity.
    if matches!(
        path,
        "/api/v1/agents/{id}/usage" | "/api/v1/agents/{id}/resources"
    ) {
        return Some(Lane::Observation);
    }
    // Descriptor polls never return chart series or report pages. Those have
    // larger existing contracts and must not inherit this small response cap.
    if matches!(
        path,
        "/api/v1/usage/metrics/view"
            | "/api/v1/admin/usage/metrics/view"
            | "/api/v1/usage/metrics/reports/{report}"
            | "/api/v1/admin/usage/metrics/reports/{report}"
    ) {
        return Some(Lane::Report);
    }
    if path == "/api/v1/documents" || path.starts_with("/api/v1/documents/") {
        return Some(Lane::Bulk);
    }
    matches!(
        path,
        "/api/v1/account/privacy-export"
            | "/api/v1/account/privacy-export/agents/{id}/transcript"
            | "/api/v1/agents/{id}/events"
            | "/api/v1/agents/{id}/transcript"
            | "/api/v1/agents/{id}/history"
            | "/api/v1/agents/{id}/artifacts"
            | "/api/v1/agents/{id}/artifacts/{artifact_id}"
            | "/api/v1/agents/{id}/artifacts/{artifact_id}/download"
            | "/api/v1/agents/{id}/files/tree"
            | "/api/v1/agents/{id}/files/content"
            | "/api/v1/agents/{id}/files/download"
    )
    .then_some(Lane::Bulk)
}
