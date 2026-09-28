//! Optional provider-owned instance selection before durable turn admission.
use crate::AuthenticatedResourceCaller;
use layer2_core::{sandbox::agent_io::ExternalEventContext, AgentId};

#[derive(Debug, Clone, Copy)]
pub enum BindingRouteError {
    Forbidden,
    Invalid,
    NotReady,
    Unavailable,
}

#[async_trait::async_trait]
pub trait BindingEventRouter: Send + Sync {
    /// Implementations must authenticate the original event before choosing an
    /// instance. The transport payload and reply reference remain unchanged.
    async fn route(
        &self,
        caller: &AuthenticatedResourceCaller,
        event: &mut ExternalEventContext,
    ) -> Result<Option<AgentId>, BindingRouteError>;
}

pub(super) fn error(value: BindingRouteError) -> axum::response::Response {
    use axum::http::{HeaderValue, StatusCode};
    let (status, code, detail) = match value {
        BindingRouteError::Forbidden => (
            StatusCode::FORBIDDEN,
            "event_identity_invalid",
            "The original event could not be authenticated for this binding.",
        ),
        BindingRouteError::Invalid => (
            StatusCode::BAD_REQUEST,
            "event_identity_unsupported",
            "This provider session policy requires a verified human source identity.",
        ),
        BindingRouteError::NotReady => (
            StatusCode::TOO_MANY_REQUESTS,
            "conversation_not_ready",
            "The provider-selected agent is preparing.",
        ),
        BindingRouteError::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_unavailable",
            "The provider cannot resolve this event now.",
        ),
    };
    let mut response = super::error::error(status, code, detail, Some("event_context"));
    if matches!(value, BindingRouteError::NotReady) {
        response
            .headers_mut()
            .insert("retry-after", HeaderValue::from_static("2"));
    }
    response
}
