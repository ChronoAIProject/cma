//! Conversation management translates endpoint-specific readiness into typed outcomes.
use super::{
    client::{acquire, transport_error},
    ProviderClient, ProviderError,
};
use cmaeg_core::{
    channel::Secret,
    ids::{BindingId, ConversationId, ProviderSlug},
};
use serde_json::json;

impl ProviderClient {
    pub(super) async fn conversation_request(
        &self,
        provider: &ProviderSlug,
        binding_id: &BindingId,
        conversation_id: &ConversationId,
        agent_key: &Secret,
        delete: bool,
        generation: Option<u64>,
    ) -> Result<(), ProviderError> {
        self.record_worker_key(agent_key).await?;
        let handle = self.handle(provider)?;
        let _permit = acquire(&handle.inflight).await?;
        let url = handle
            .entry
            .url(
                "conversation",
                &[
                    ("binding_id", binding_id.as_str()),
                    ("conversation_id", conversation_id.as_str()),
                ],
            )
            .map_err(|error| ProviderError::InvalidRequest(error.to_string()))?;
        let bearer = self.management_bearer(provider, &url, agent_key).await?;
        let mut request = if delete {
            handle.client.delete(url)
        } else {
            handle.client.put(url).json(&json!({}))
        };
        request = request.bearer_auth(bearer.as_str());
        if let Some(generation) = generation {
            request = request.header("x-cma-binding-generation", generation);
        }
        let request = request.send();
        let response = tokio::time::timeout(self.request_timeout, request)
            .await
            .map_err(|_| ProviderError::Unavailable("provider request timed out".into()))?
            .map_err(transport_error)?;
        match self.success_or_error(response).await {
            // CMA acknowledges the binding reservation before its sandbox is
            // ready. Only this explicit creation refusal is retryable; delete,
            // other providers, other conflicts and unknown outcomes retain
            // their existing classification.
            Err(ProviderError::Rejected { status: 409, code })
                if !delete && provider.as_str() == "cma_codex" && code == "binding_not_ready" =>
            {
                Err(ProviderError::Busy { retry_after: None })
            }
            result => result.map(|_| ()),
        }
    }
}
