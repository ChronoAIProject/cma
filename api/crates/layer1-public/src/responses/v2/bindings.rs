use super::{error, ResponsesV2State};
use crate::AuthenticatedResourceCaller;
use axum::{http::StatusCode, response::Response};
use layer2_core::{resource_access::CredentialSource, sandbox::binding::BindingError, AgentId};

impl ResponsesV2State {
    pub(super) async fn resolve_conversation(
        &self,
        auth: &AuthenticatedResourceCaller,
        id: &AgentId,
    ) -> Result<AgentId, Box<Response>> {
        if auth.context.credential_source != CredentialSource::Binding {
            return Ok(id.clone());
        }
        let bindings = self
            .bindings
            .as_ref()
            .ok_or_else(|| Box::new(error::unavailable()))?;
        let principal = bindings
            .authenticate(&auth.context.credential.credential_id)
            .await
            .map_err(binding_error)?
            .ok_or_else(not_found)?;
        if principal.caller() != auth.context.caller {
            return Err(not_found());
        }
        let proof = bindings
            .resolve(&principal, id.as_str())
            .await
            .map_err(binding_error)?;
        Ok(proof.conversation.agent_id)
    }
}
fn not_found() -> Box<Response> {
    Box::new(error::error(
        StatusCode::NOT_FOUND,
        "conversation_not_found",
        "The conversation is not available to this binding.",
        Some("conversation"),
    ))
}
fn binding_error(error: BindingError) -> Box<Response> {
    match error {
        BindingError::NotFound | BindingError::Forbidden => not_found(),
        // A reserved conversation is not admissible until its Agent is ready.
        // Use the provider contract's retryable capacity response so a gateway
        // keeps the original event and idempotency key during preparation.
        BindingError::Conflict => {
            let mut response = super::error::error(
                StatusCode::TOO_MANY_REQUESTS,
                "conversation_not_ready",
                "The binding conversation is not ready.",
                Some("conversation"),
            );
            // Worktree checkout and provider startup may take over a minute.
            // Pace readiness retries within the default create-request budget.
            response
                .headers_mut()
                .insert("retry-after", axum::http::HeaderValue::from_static("6"));
            Box::new(response)
        }
        _ => Box::new(super::error::unavailable()),
    }
}
