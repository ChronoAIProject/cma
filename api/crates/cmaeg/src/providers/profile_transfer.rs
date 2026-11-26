//! Exchange one exact immutable-profile transfer receipt with CMA.
use super::{
    reset::{bounded_body, reset_response_error},
    ProviderClient, ProviderError,
};
use cmaeg_core::ids::{BindingId, ConversationId, ProviderSlug};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProfileTransferRequest {
    pub destination_binding_id: String,
    pub expected_agent_id: String,
    pub expected_generation: u64,
    pub expected_destination_generation: u64,
    pub expected_incarnation: u64,
    pub owner: String,
    pub organization: bool,
    pub request_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProfileTransfer {
    pub source_binding_id: String,
    pub source_generation: u64,
    pub source_incarnation: u64,
    pub source_agent_id: String,
    pub source_conversation_id: String,
    pub conversation_id: String,
    pub destination_binding_id: String,
    pub destination_generation: u64,
    pub destination_agent_id: String,
    pub destination_worktree_id: String,
    pub state: String,
}

impl ProviderClient {
    pub(crate) async fn transfer_profile(
        &self,
        provider: &ProviderSlug,
        binding: &BindingId,
        conversation: &ConversationId,
        key: &str,
        body: &ProfileTransferRequest,
    ) -> Result<ProfileTransfer, ProviderError> {
        let handle = self.handle(provider)?;
        let invalid = || ProviderError::InvalidRequest("invalid profile transfer path".into());
        let mut url = handle
            .entry
            .url(
                "conversation",
                &[
                    ("binding_id", binding.as_str()),
                    ("conversation_id", conversation.as_str()),
                ],
            )
            .map_err(|_| invalid())?;
        url.path_segments_mut()
            .map_err(|_| invalid())?
            .push("profile-transfers");
        if url.path()
            != format!(
                "/api/v1/cmaeg/bindings/{binding}/conversations/{conversation}/profile-transfers"
            )
        {
            return Err(invalid());
        }
        let bearer = self.reset_bearer(provider, &url).await?;
        let _permit = super::client::acquire(&handle.inflight).await?;
        let response = handle
            .client
            .post(url)
            .bearer_auth(bearer.as_str())
            .header("idempotency-key", key)
            .json(body)
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(|_| {
                ProviderError::Unavailable(
                    "profile transfer response unavailable; retry the same key".into(),
                )
            })?;
        let status = response.status();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .map(std::time::Duration::from_secs);
        let bytes = tokio::time::timeout(self.request_timeout, bounded_body(response))
            .await
            .map_err(|_| {
                ProviderError::Unavailable("profile transfer response unavailable".into())
            })??;
        if status != reqwest::StatusCode::OK {
            return Err(reset_response_error(status, retry_after, &bytes));
        }
        let result: ProfileTransfer = serde_json::from_slice(&bytes).map_err(|_| {
            ProviderError::ResponseMalformed("invalid profile transfer receipt".into())
        })?;
        let valid_id = |value: &str| !value.is_empty() && value.len() <= 128;
        if result.source_binding_id != binding.as_str()
            || result.conversation_id != conversation.as_str()
            || result.source_generation != body.expected_generation
            || result.source_incarnation != body.expected_incarnation
            || result.source_agent_id != body.expected_agent_id
            || result.destination_binding_id != body.destination_binding_id
            || result.destination_generation != body.expected_destination_generation
            || result.destination_agent_id == result.source_agent_id
            || !valid_id(&result.source_conversation_id)
            || !valid_id(&result.destination_agent_id)
            || !valid_id(&result.destination_worktree_id)
            || result.state != "reserved"
        {
            return Err(ProviderError::ResponseMalformed(
                "profile transfer identity mismatch".into(),
            ));
        }
        Ok(result)
    }
}
