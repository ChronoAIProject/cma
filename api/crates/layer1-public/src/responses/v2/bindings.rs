use super::{error, ResponsesV2State};
use crate::AuthenticatedResourceCaller;
use axum::{http::StatusCode, response::Response};
use layer2_core::{resource_access::CredentialSource, sandbox::binding::BindingError, AgentId};

impl ResponsesV2State {
    pub(super) async fn submission_policy(
        &self,
        auth: &AuthenticatedResourceCaller,
        expected: Option<u64>,
    ) -> Result<layer2_core::sandbox::binding::BindingSubmissionPolicy, Box<Response>> {
        let bindings = self
            .bindings
            .as_ref()
            .ok_or_else(|| Box::new(error::unavailable()))?;
        bindings
            .submission_policy(&auth.context.credential.credential_id, expected)
            .await
            .map_err(|value| match value {
                layer2_core::sandbox::binding::BindingError::Conflict => Box::new(error::error(
                    StatusCode::CONFLICT,
                    "binding_generation_changed",
                    "This event belongs to an unavailable binding generation. Send a new message.",
                    Some("binding_generation"),
                )),
                other => binding_error(other),
            })
    }

    pub(super) async fn resolve_conversation(
        &self,
        auth: &AuthenticatedResourceCaller,
        id: &AgentId,
        generation: Option<u64>,
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
        if principal.caller() != auth.context.caller
            || generation.is_some_and(|expected| principal.generation() != expected)
        {
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
        BindingError::AdmissionPaused => Box::new(super::error::admission_paused()),
        BindingError::GlobalCapacityReached => Box::new(super::error::home_capacity()),
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

#[cfg(test)]
mod admission_pause_tests {
    use super::*;

    #[tokio::test]
    async fn binding_home_capacity_refusal_preserves_safe_response_guidance() {
        let response = *binding_error(BindingError::GlobalCapacityReached);
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()["retry-after"], "60");
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "sandbox_capacity_unavailable");
        assert_eq!(
            body["error"]["message"],
            layer2_core::admission::CapacityKind::RetainedHomes.guidance()
        );
        assert_eq!(body["error"].as_object().unwrap().len(), 4);
        assert!(!body.to_string().contains("limit"));
        assert!(!body.to_string().contains("count"));
    }

    #[tokio::test]
    async fn binding_conversation_preparation_paces_operator_pause_retries() {
        let response = *binding_error(BindingError::AdmissionPaused);
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
