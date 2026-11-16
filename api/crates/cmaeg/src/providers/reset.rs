//! Synchronous provider-owned reset; the gateway retains no operation state.
use super::{ProviderClient, ProviderError};
use cmaeg_core::{
    channel::Secret,
    ids::{BindingId, ConversationId, ProviderSlug},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ConversationReset {
    pub binding_id: String,
    pub generation: u64,
    pub conversation_id: String,
    pub incarnation: u64,
    pub previous_agent_id: String,
    pub previous_conversation_id: String,
    pub successor_agent_id: String,
    pub state: String,
}
impl ProviderClient {
    #[allow(clippy::too_many_arguments)]
    pub async fn reset_conversation(
        &self,
        provider: &ProviderSlug,
        binding: &BindingId,
        conversation: &ConversationId,
        credential: &Secret,
        key: &str,
        body: serde_json::Value,
    ) -> Result<ConversationReset, ProviderError> {
        let handle = self.handle(provider)?;
        let mut url = handle
            .entry
            .url(
                "conversation",
                &[
                    ("binding_id", binding.as_str()),
                    ("conversation_id", conversation.as_str()),
                ],
            )
            .map_err(|_| ProviderError::InvalidRequest("invalid conversation reset path".into()))?;
        url.path_segments_mut()
            .map_err(|_| ProviderError::InvalidRequest("invalid conversation reset path".into()))?
            .push("resets");
        let bearer = self.management_bearer(provider, &url, credential).await?;
        let _permit = super::client::acquire(&handle.inflight).await?;
        let response = handle
            .client
            .post(url)
            .bearer_auth(bearer.as_str())
            .header("idempotency-key", key)
            .json(&body)
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(|_| {
                ProviderError::Unavailable(
                    "conversation reset response unavailable; retry the same key".into(),
                )
            })?;
        let mut response = self.success_or_error(response).await?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            ProviderError::Unavailable("conversation reset response unavailable".into())
        })? {
            if bytes.len() + chunk.len() > 16384 {
                return Err(ProviderError::ResponseMalformed(
                    "conversation reset exceeds limit".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let result: ConversationReset = serde_json::from_slice(&bytes)
            .map_err(|_| ProviderError::ResponseMalformed("invalid conversation reset".into()))?;
        if result.binding_id != binding.as_str()
            || result.conversation_id != conversation.as_str()
            || result.generation == 0
            || result.incarnation == 0
            || result.previous_agent_id != body["expected_agent_id"].as_str().unwrap_or_default()
            || result.successor_agent_id.is_empty()
            || result.successor_agent_id == result.previous_agent_id
            || result.state != "reserved"
        {
            return Err(ProviderError::ResponseMalformed(
                "conversation reset identity mismatch".into(),
            ));
        }
        Ok(result)
    }
}
