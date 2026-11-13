//! Full event context travels atomically with a turn and is read from its provider.
use super::{context_retry, ContextFailure, ProviderClient, ProviderError};
use cmaeg_core::{
    activity::Activity,
    channel::Secret,
    ids::{BindingId, ChannelId, ConversationId, EventId, ProviderSlug},
    seal::EventRef,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EventContextSnapshot {
    pub schema_version: u16,
    pub channel_id: ChannelId,
    pub conversation_id: ConversationId,
    pub activity: Activity,
    pub event_ref: EventRef,
}
impl EventContextSnapshot {
    pub fn validate(&self) -> Result<(), ProviderError> {
        if self.schema_version != 1
            || !self.conversation_id.is_wellformed()
            || self.event_ref.as_str().len() > 4096
        {
            return Err(ProviderError::InvalidRequest(
                "invalid event context".into(),
            ));
        }
        self.activity
            .validate()
            .map_err(|_| ProviderError::InvalidRequest("invalid context activity".into()))
    }
}
impl ProviderClient {
    /// The suffix extends the configured conversation resource on the same allowed origin.
    pub async fn event_context(
        &self,
        provider: &ProviderSlug,
        binding: &BindingId,
        conversation: &ConversationId,
        event: &EventId,
        agent_key: &Secret,
    ) -> Result<EventContextSnapshot, ProviderError> {
        self.find_event_context(provider, binding, conversation, event, agent_key)
            .await?
            .ok_or_else(|| ProviderError::Unavailable("provider context is unavailable".into()))
    }

    /// Read admitted context under the current binding authority. Only an
    /// explicit 404 proves absence; authentication and transport failures do not.
    pub async fn find_event_context(
        &self,
        provider: &ProviderSlug,
        binding: &BindingId,
        conversation: &ConversationId,
        event: &EventId,
        agent_key: &Secret,
    ) -> Result<Option<EventContextSnapshot>, ProviderError> {
        self.find_event_context_once(provider, binding, conversation, event, agent_key)
            .await
            .map_err(|failure| failure.error)
    }

    pub(crate) async fn find_event_context_with_retry(
        &self,
        provider: &ProviderSlug,
        binding: &BindingId,
        conversation: &ConversationId,
        event: &EventId,
        agent_key: &Secret,
    ) -> Result<Option<EventContextSnapshot>, ContextFailure> {
        context_retry::retry(|| {
            self.find_event_context_once(provider, binding, conversation, event, agent_key)
        })
        .await
    }

    async fn find_event_context_once(
        &self,
        provider: &ProviderSlug,
        binding: &BindingId,
        conversation: &ConversationId,
        event: &EventId,
        agent_key: &Secret,
    ) -> Result<Option<EventContextSnapshot>, ContextFailure> {
        self.record_worker_key(agent_key).await?;
        if matches!(event.as_str(), "." | "..") {
            return Err(ProviderError::InvalidRequest("invalid event identifier".into()).into());
        }
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
            .map_err(|_| ProviderError::InvalidRequest("invalid context path".into()))?;
        url.path_segments_mut()
            .map_err(|_| ProviderError::InvalidRequest("invalid context path".into()))?
            .push("events")
            .push(event.as_str());
        let mut response = handle
            .client
            .get(url)
            .bearer_auth(agent_key.as_str())
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .map_err(ContextFailure::transport)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if matches!(response.status().as_u16(), 401 | 403) {
            return Err(ProviderError::Rejected {
                status: response.status().as_u16(),
                code: "provider_context_not_authorized".into(),
            }
            .into());
        }
        if !response.status().is_success() {
            return Err(ContextFailure::response(
                response.status().as_u16(),
                response.headers().get(reqwest::header::RETRY_AFTER),
            ));
        }
        if response.content_length().is_some_and(|len| len > 1_048_576) {
            return Err(
                ProviderError::ResponseMalformed("context response exceeds limit".into()).into(),
            );
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(ContextFailure::transport)? {
            if bytes.len() + chunk.len() > 1_048_576 {
                return Err(ProviderError::ResponseMalformed(
                    "context response exceeds limit".into(),
                )
                .into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let snapshot: EventContextSnapshot = serde_json::from_slice(&bytes)
            .map_err(|_| ProviderError::ResponseMalformed("invalid provider context".into()))?;
        snapshot.validate()?;
        if snapshot.conversation_id != *conversation || snapshot.activity.event_id != *event {
            return Err(ProviderError::ResponseMalformed(
                "provider context identity mismatch".into(),
            )
            .into());
        }
        Ok(Some(snapshot))
    }
}
