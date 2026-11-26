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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConversationCatalog {
    pub binding_id: String,
    pub generation: u64,
    pub revision: u64,
    pub transport_conversations: Vec<String>,
}

impl ProviderClient {
    pub(crate) async fn conversation_catalog(
        &self,
        provider: &ProviderSlug,
        binding: &BindingId,
        credential: &Secret,
    ) -> Result<ConversationCatalog, ProviderError> {
        if provider.as_str() != "cma_codex" {
            return Err(ProviderError::InvalidRequest(
                "conversation catalog requires CMA".into(),
            ));
        }
        let handle = self.handle(provider)?;
        let _permit = super::client::acquire(&handle.inflight).await?;
        let mut url = handle
            .entry
            .url("binding", &[("binding_id", binding.as_str())])
            .map_err(|_| ProviderError::InvalidRequest("invalid binding path".into()))?;
        url.path_segments_mut()
            .map_err(|_| ProviderError::InvalidRequest("invalid binding path".into()))?
            .pop_if_empty()
            .push("conversations");
        let bearer = self.management_bearer(provider, &url, credential).await?;
        let response = handle
            .client
            .get(url)
            .bearer_auth(bearer.as_str())
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(|_| ProviderError::Unavailable("conversation catalog unavailable".into()))?;
        let mut response = self.success_or_error(response).await?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| ProviderError::Unavailable("conversation catalog unavailable".into()))?
        {
            if bytes.len() + chunk.len() > 768 * 1024 {
                return Err(ProviderError::ResponseMalformed(
                    "conversation catalog exceeds bound".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let catalog: ConversationCatalog = serde_json::from_slice(&bytes)
            .map_err(|_| ProviderError::ResponseMalformed("invalid conversation catalog".into()))?;
        if catalog.binding_id != binding.as_str()
            || catalog.generation == 0
            || catalog.revision == 0
            || catalog.transport_conversations.len() > 1024
            || catalog
                .transport_conversations
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
            || catalog
                .transport_conversations
                .iter()
                .any(|id| ConversationId::new(id).is_err())
        {
            return Err(ProviderError::ResponseMalformed(
                "invalid conversation catalog identity".into(),
            ));
        }
        Ok(catalog)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DesiredProfileDrift {
    pub profile_digest: String,
    pub prompt: bool,
    pub skills: bool,
    pub model: bool,
    pub service_scope: bool,
}

impl ProviderClient {
    pub(crate) async fn desired_profile_drift(
        &self,
        provider: &ProviderSlug,
        binding: &BindingId,
        conversation: &ConversationId,
        credential: &Secret,
        profile: &cmaeg_core::profile::EventWorkerProfile,
    ) -> Result<Option<DesiredProfileDrift>, ProviderError> {
        if provider.as_str() != "cma_codex" {
            return Err(ProviderError::InvalidRequest(
                "profile drift requires CMA".into(),
            ));
        }
        let handle = self.handle(provider)?;
        let _permit = super::client::acquire(&handle.inflight).await?;
        let mut url = handle
            .entry
            .url(
                "conversation",
                &[
                    ("binding_id", binding.as_str()),
                    ("conversation_id", conversation.as_str()),
                ],
            )
            .map_err(|_| ProviderError::InvalidRequest("invalid conversation path".into()))?;
        url.path_segments_mut()
            .map_err(|_| ProviderError::InvalidRequest("invalid conversation path".into()))?
            .pop_if_empty()
            .push("profile-drift");
        let bearer = self.management_bearer(provider, &url, credential).await?;
        let digest = profile.digest();
        let mut response = self
            .success_or_error(
                handle
                    .client
                    .post(url)
                    .bearer_auth(bearer.as_str())
                    .json(&serde_json::json!({"profile":profile,"profile_digest":digest}))
                    .timeout(self.request_timeout)
                    .send()
                    .await
                    .map_err(|_| ProviderError::Unavailable("profile drift unavailable".into()))?,
            )
            .await?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| ProviderError::Unavailable("profile drift unavailable".into()))?
        {
            if bytes.len() + chunk.len() > 16_384 {
                return Err(ProviderError::ResponseMalformed(
                    "profile drift exceeds bound".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let view: Option<DesiredProfileDrift> = serde_json::from_slice(&bytes)
            .map_err(|_| ProviderError::ResponseMalformed("invalid profile drift".into()))?;
        if view
            .as_ref()
            .is_some_and(|value| value.profile_digest != digest)
        {
            return Err(ProviderError::ResponseMalformed(
                "profile drift identity mismatch".into(),
            ));
        }
        Ok(view)
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
