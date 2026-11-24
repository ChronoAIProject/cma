//! Request work classes and exact public route policy.

use axum::{
    extract::{MatchedPath, Request},
    http::Method,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequestTier {
    Health,
    Default,
    Create,
    Input,
    Control,
}

impl RequestTier {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Health => "health",
            Self::Default => "default",
            Self::Create => "create",
            Self::Input => "input",
            Self::Control => "control",
        }
    }
}

pub(super) fn request_tier(
    request: &Request,
    response_inputs: bool,
    controls: bool,
) -> RequestTier {
    let matched_path = request.extensions().get::<MatchedPath>();
    let path = matched_path
        .map(MatchedPath::as_str)
        .unwrap_or_else(|| request.uri().path());
    if matches!(request.method(), &Method::GET | &Method::HEAD)
        && matches!(path, "/livez" | "/ready")
    {
        return RequestTier::Health;
    }
    if path == layer1_public::rest::creation_outcomes::LOOKUP_PATH
        || request.method() != Method::POST
        || matches!(
            path,
            "/api/v1/profile-authoring/repositories/resolve"
                | "/auth/connections/check"
                | "/auth/connections/repository-access"
        )
    {
        return RequestTier::Default;
    }
    // Recovery and controls of admitted work take precedence over Input.
    // Only exact router metadata can grant this independent allowance.
    if controls
        && matched_path.is_some()
        && matches!(
            path,
            "/api/v1/agents/{id}/stop"
                | "/api/v1/agents/{id}/interrupt"
                | "/api/v1/agents/{id}/approvals/{request_id}"
                | "/api/v1/agents/{id}/access-requests/{request_id}/approve"
                | "/api/v1/agents/{id}/access-requests/{request_id}/deny"
                | "/api/v1/launches/{id}/cancel"
                | "/api/v1/responses/{response_id}/cancel"
                | "/api/v2/agents/{agent_id}/responses/{response_id}/interrupt"
                | "/api/v2/agents/{agent_id}/responses/{response_id}/controls"
                | "/api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation"
                | "/api/v1/sandboxes/{id}/pause"
                | "/api/v1/sandboxes/{id}/commands"
                | "/api/v1/sandboxes/{id}/revive"
                | "/api/v1/agents/{id}/capacity-recovery/resume"
        )
    {
        // Recovery routes and Sandbox command Revive retain domain admission.
        return RequestTier::Control;
    }
    // Parameterized paths must be exact registered MatchedPath templates.
    // The disabled branch preserves the prior suffix-based policy.
    let input = if response_inputs {
        // Binding event routing can provision a source-user conversation.
        request
            .extensions()
            .get::<layer1_public::AuthMethod>()
            .is_none()
            && (matches!(
                path,
                "/api/v2/responses"
                    | "/api/v2/agents/{agent_id}/responses"
                    | "/api/v1/responses"
                    | "/api/v1/agui/run"
                    | "/api/v1/agents/{id}/input"
                    | "/api/v1/agents/{id}/steer"
                    | "/api/v2/agents/{agent_id}/responses/{response_id}/queued-input"
            ) || (!controls
                && path == "/api/v2/agents/{agent_id}/responses/{response_id}/interrupt"))
    } else {
        path.split('/').rfind(|segment| !segment.is_empty()) == Some("input")
    };
    if input {
        return RequestTier::Input;
    }
    RequestTier::Create
}

/// Routes that never require identity keep a network budget even when a
/// browser happens to attach a cookie. Otherwise the same public operation
/// changes buckets based on ambient credentials and can be used to exhaust a
/// subject's protected-route allowance.
pub(super) fn uses_client_operation_key(path: &str) -> bool {
    // These authenticated endpoints coordinate one owner's connections. Keep
    // users behind the same network from consuming each other's login budget.
    if matches!(
        path,
        "/auth/connections/check"
            | "/auth/connections/start"
            | "/auth/connections/cancel"
            | "/auth/connections/repository-access"
    ) {
        return false;
    }
    path == "/openapi.json"
        || path == "/api/v1/conformance"
        || is_public_trigger_preview(path)
        || path == "/.well-known/agent-card.json"
        || path == "/auth"
        || path.starts_with("/auth/")
}

pub(super) fn is_rest_path(path: &str) -> bool {
    if path == "/openapi.json" {
        return true;
    }
    if path != "/api/v1" && path != "/api/v1/" && !path.starts_with("/api/v1/") {
        return false;
    }
    path != "/api/v1/agui"
        && !path.starts_with("/api/v1/agui/")
        && path != "/api/v1/mcp"
        && path != "/api/v1/a2a"
        && path != "/api/v1/responses"
}

pub(super) fn is_public_trigger_preview(path: &str) -> bool {
    exact_path_segments(path, &["api", "v1", "triggers", "public", "*"])
        || exact_path_segments(path, &["api", "v1", "triggers", "public", "*", "badge.svg"])
}

pub(super) fn exact_path_segments(path: &str, expected: &[&str]) -> bool {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    expected.iter().all(|expected| {
        segments
            .next()
            .is_some_and(|actual| *expected == "*" || actual == *expected)
    }) && segments.next().is_none()
}
