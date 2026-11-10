//! Optional provider-owned instance selection before durable turn admission.
use crate::AuthenticatedResourceCaller;
use layer2_core::{sandbox::agent_io::ExternalEventContext, AgentId};

#[derive(Debug, Clone, Copy)]
pub enum BindingRouteError {
    Forbidden,
    Invalid,
    NotReady,
    Unavailable,
    SenderInvalid,
    SenderUnsupported,
    SenderUnavailable,
}

#[async_trait::async_trait]
pub trait BindingEventRouter: Send + Sync {
    /// Pure projection for comparison with an already admitted complete command.
    /// This does not attest a new event or authorize a new admission.
    fn sender_line(&self, _event: &ExternalEventContext) -> Result<String, BindingRouteError> {
        Err(BindingRouteError::SenderUnsupported)
    }
    /// Attest the complete original event and render one deterministic first line.
    async fn verified_sender_line(
        &self,
        _caller: &AuthenticatedResourceCaller,
        _event: &ExternalEventContext,
        _generation: u64,
    ) -> Result<String, BindingRouteError> {
        Err(BindingRouteError::SenderUnavailable)
    }
    /// The interface captured this generation before resolving any alias.
    async fn route_at_generation(
        &self,
        caller: &AuthenticatedResourceCaller,
        event: &mut ExternalEventContext,
        _generation: u64,
    ) -> Result<Option<AgentId>, BindingRouteError> {
        self.route(caller, event).await
    }

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
        BindingRouteError::SenderInvalid => (
            StatusCode::FORBIDDEN,
            "verified_sender_context_invalid",
            "The gateway did not attest the original event for this binding.",
        ),
        BindingRouteError::SenderUnsupported => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "verified_sender_context_unsupported",
            "The verified sender context cannot be represented within the input contract.",
        ),
        BindingRouteError::SenderUnavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "verified_sender_context_unavailable",
            "Sender verification is temporarily unavailable; no turn was admitted.",
        ),
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
    if matches!(value, BindingRouteError::SenderUnavailable) {
        response
            .headers_mut()
            .insert("retry-after", HeaderValue::from_static("5"));
    }
    response
}
