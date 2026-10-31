use super::{ProviderClient, ProviderError};
use cmaeg_core::{
    channel::Secret,
    ids::{BindingId, ConversationId, ProviderSlug},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ConversationInspection {
    pub binding_id: String,
    pub generation: u64,
    pub conversation_id: String,
    pub agent_id: String,
    pub state: String,
    pub agent_status: Option<String>,
}

impl ProviderClient {
    pub async fn inspect_conversation(
        &self,
        provider: &ProviderSlug,
        binding: &BindingId,
        conversation: &ConversationId,
        credential: &Secret,
    ) -> Result<ConversationInspection, ProviderError> {
        if provider.as_str() != "cma_codex" {
            return Err(ProviderError::InvalidRequest(
                "conversation inspection is unsupported for this provider".into(),
            ));
        }
        let handle = self.handle(provider)?;
        let _permit = super::client::acquire(&handle.inflight).await?;
        let url = handle
            .entry
            .url(
                "conversation",
                &[
                    ("binding_id", binding.as_str()),
                    ("conversation_id", conversation.as_str()),
                ],
            )
            .map_err(|_| ProviderError::InvalidRequest("invalid conversation path".into()))?;
        let bearer = self.management_bearer(provider, &url, credential).await?;
        let response = handle
            .client
            .get(url)
            .bearer_auth(bearer.as_str())
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(|_| {
                ProviderError::Unavailable("conversation inspection unavailable".into())
            })?;
        let mut response = self.success_or_error(response).await?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| ProviderError::Unavailable("conversation inspection unavailable".into()))?
        {
            if bytes.len() + chunk.len() > 16_384 {
                return Err(ProviderError::ResponseMalformed(
                    "conversation inspection exceeds limit".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let result: ConversationInspection = serde_json::from_slice(&bytes).map_err(|_| {
            ProviderError::ResponseMalformed("invalid conversation inspection".into())
        })?;
        if result.binding_id != binding.as_str()
            || result.generation == 0
            || result.agent_id.is_empty()
            || (result.conversation_id != conversation.as_str()
                && result.conversation_id != "existing_agent")
        {
            return Err(ProviderError::ResponseMalformed(
                "conversation inspection identity mismatch".into(),
            ));
        }
        Ok(result)
    }
}
