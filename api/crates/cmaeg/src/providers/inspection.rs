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
    #[serde(default)]
    #[schema(required = false)]
    pub incarnation: u64,
    pub agent_id: String,
    pub state: String,
    pub agent_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_drift: Option<BindingProfileDrift>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BindingProfileDrift {
    pub assigned_profile_digest: String,
    pub prompt: bool,
    pub skills: bool,
    pub model: bool,
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inspection_accepts_legacy_zero_and_positive_incarnations() {
        let mut body = serde_json::json!({"binding_id":"binding","generation":1,
            "conversation_id":"transport","agent_id":"agent","state":"ready","agent_status":"ready"});
        assert_eq!(
            serde_json::from_value::<ConversationInspection>(body.clone())
                .unwrap()
                .incarnation,
            0
        );
        for incarnation in [0, 1, u64::MAX] {
            body["incarnation"] = incarnation.into();
            assert_eq!(
                serde_json::from_value::<ConversationInspection>(body.clone())
                    .unwrap()
                    .incarnation,
                incarnation
            );
        }
        for invalid in [
            serde_json::json!(-1),
            serde_json::json!("1"),
            serde_json::Value::Null,
        ] {
            body["incarnation"] = invalid;
            assert!(serde_json::from_value::<ConversationInspection>(body.clone()).is_err());
        }
    }
}
