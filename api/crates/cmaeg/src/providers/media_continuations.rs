//! Narrow existing-Agent media calls. All target IDs come from the immutable
//! routed parent; redirects are disabled by the provider client pool.
use super::{
    client::{acquire, transport_error},
    ProviderClient, ProviderError,
};
use crate::{
    ingress::routing_store::StoredTarget,
    turns::routed::{MediaContinuationReceipt, MediaUploadReceipt},
};
use cmaeg_core::{
    channel::Secret,
    media_continuation::{sha256, MediaArtifactClaim, MediaContinuationKind},
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
const RESPONSE_BOUND: usize = 16 * 1024;
#[cfg(test)]
#[path = "media_continuation_tests.rs"]
mod tests;
#[derive(Serialize)]
struct Supplement<'a> {
    gateway_binding_generation: Option<u64>,
    event_expires_at: i64,
    artifact: &'a MediaArtifactClaim,
    derived_text: &'a str,
}
#[derive(Deserialize)]
struct Admission {
    command_id: String,
    response_id: String,
    response_url: String,
    status: String,
    queue_deadline: String,
    metadata: std::collections::BTreeMap<String, String>,
}
impl ProviderClient {
    #[cfg(test)]
    pub(crate) fn with_cma_origin_fixture(mut self, origin: &str) -> Self {
        self.management_auth = Some(
            super::management_auth::CmaManagementAuth::new(
                origin,
                Secret::new("unused-service-fixture"),
            )
            .unwrap(),
        );
        self
    }

    fn media_url(
        &self,
        target: &StoredTarget,
        index: u8,
        action: &str,
    ) -> Result<url::Url, ProviderError> {
        let invalid = || {
            ProviderError::InvalidRequest(
                "media continuation requires the configured CMA origin and existing Agent target"
                    .into(),
            )
        };
        if target.launch.provider.slug.as_str() != "cma_codex"
            || target.launch.provider.target.is_profile()
            || index >= 32
        {
            return Err(invalid());
        }
        let handle = self.handle(&target.launch.provider.slug)?;
        let mut url = handle
            .entry
            .url(
                "binding",
                &[("binding_id", target.launch.provider.binding_id.as_str())],
            )
            .map_err(|_| invalid())?;
        if !self.permits_preparation(&target.launch.provider.slug, &url) {
            return Err(invalid());
        }
        url.path_segments_mut().map_err(|_| invalid())?.extend([
            "conversations",
            target.launch.event.conversation_id.as_str(),
            "events",
            target.launch.event.event_id.as_str(),
            "media",
            &index.to_string(),
            action,
        ]);
        Ok(url)
    }
    pub async fn event_media_upload_receipt(
        &self,
        target: &StoredTarget,
        artifact: &MediaArtifactClaim,
        credential: &Secret,
    ) -> Result<Option<MediaUploadReceipt>, ProviderError> {
        artifact
            .validate()
            .map_err(|_| ProviderError::InvalidRequest("invalid frozen original".into()))?;
        let url = self.media_url(target, artifact.attachment_index, "attachment")?;
        let handle = self.handle(&target.launch.provider.slug)?;
        let _permit = acquire(&handle.inflight).await?;
        let response = handle
            .client
            .get(url)
            .bearer_auth(credential.as_str())
            .timeout(self.request_timeout.min(Duration::from_secs(10)))
            .send()
            .await
            .map_err(transport_error)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = self.success_or_error(response).await?;
        let receipt: MediaUploadReceipt = read_bounded(response).await?;
        receipt.validate(artifact).map_err(|_| {
            ProviderError::ResponseMalformed(
                "attachment receipt differs from frozen original".into(),
            )
        })?;
        let cmaeg_core::channel::ProviderTarget::ExistingAgent { agent_id } =
            &target.launch.provider.target
        else {
            unreachable!()
        };
        if &receipt.agent_id != agent_id {
            return Err(ProviderError::ResponseMalformed(
                "attachment recipient differs from original target".into(),
            ));
        }
        Ok(Some(receipt))
    }
    pub async fn upload_event_media(
        &self,
        target: &StoredTarget,
        generation: Option<u64>,
        artifact: &MediaArtifactClaim,
        bytes: bytes::Bytes,
        credential: &Secret,
    ) -> Result<MediaUploadReceipt, ProviderError> {
        artifact
            .validate()
            .map_err(|_| ProviderError::InvalidRequest("invalid original artifact".into()))?;
        if artifact.kind != MediaContinuationKind::Original
            || artifact.text_sha256.is_some()
            || artifact.size_bytes != bytes.len() as u64
            || artifact.source_sha256 != sha256(&bytes)
            || generation == Some(0)
        {
            return Err(ProviderError::InvalidRequest(
                "original bytes do not match frozen artifact".into(),
            ));
        }
        let mut url = self.media_url(target, artifact.attachment_index, "attachment")?;
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("name", &artifact.name)
                .append_pair("sha256", &artifact.source_sha256)
                .append_pair(
                    "event_expires_at",
                    &target.launch.event.expires_at.to_string(),
                );
            if let Some(generation) = generation {
                query.append_pair("gateway_binding_generation", &generation.to_string());
            }
        }
        let handle = self.handle(&target.launch.provider.slug)?;
        let _permit = acquire(&handle.inflight).await?;
        let response = handle
            .client
            .post(url)
            .bearer_auth(credential.as_str())
            .header(reqwest::header::CONTENT_TYPE, &artifact.media_type)
            .header(reqwest::header::CONTENT_LENGTH, artifact.size_bytes)
            .body(bytes)
            .timeout(self.request_timeout.min(Duration::from_secs(20)))
            .send()
            .await
            .map_err(transport_error)?;
        let response = self.success_or_error(response).await?;
        let receipt: MediaUploadReceipt = read_bounded(response).await?;
        receipt.validate(artifact).map_err(|_| {
            ProviderError::ResponseMalformed(
                "attachment receipt differs from frozen original".into(),
            )
        })?;
        let cmaeg_core::channel::ProviderTarget::ExistingAgent { agent_id } =
            &target.launch.provider.target
        else {
            unreachable!()
        };
        if &receipt.agent_id != agent_id {
            return Err(ProviderError::ResponseMalformed(
                "attachment recipient differs from original target".into(),
            ));
        }
        Ok(receipt)
    }
    pub async fn supplement_event_media(
        &self,
        target: &StoredTarget,
        generation: Option<u64>,
        artifact: &MediaArtifactClaim,
        derived_text: &str,
        credential: &Secret,
    ) -> Result<MediaContinuationReceipt, ProviderError> {
        artifact
            .validate_derived_text(derived_text)
            .map_err(|_| ProviderError::InvalidRequest("invalid media supplement".into()))?;
        if artifact.text_sha256.is_none() || generation == Some(0) {
            return Err(ProviderError::InvalidRequest(
                "unfrozen media supplement".into(),
            ));
        }
        let url = self.media_url(target, artifact.attachment_index, "supplement")?;
        let handle = self.handle(&target.launch.provider.slug)?;
        let _permit = acquire(&handle.inflight).await?;
        let response = handle
            .client
            .post(url)
            .bearer_auth(credential.as_str())
            .json(&Supplement {
                gateway_binding_generation: generation,
                event_expires_at: target.launch.event.expires_at,
                artifact,
                derived_text,
            })
            .timeout(self.request_timeout.min(Duration::from_secs(20)))
            .send()
            .await
            .map_err(transport_error)?;
        let response = self.success_or_error(response).await?;
        let admission: Admission = read_bounded(response).await?;
        if admission.response_url != format!("/api/v2/responses/{}", admission.response_id)
            || !matches!(
                admission.status.as_str(),
                "queued" | "in_progress" | "waiting_action" | "completed" | "failed" | "cancelled"
            )
            || time::OffsetDateTime::parse(
                &admission.queue_deadline,
                &time::format_description::well_known::Rfc3339,
            )
            .is_err()
        {
            return Err(ProviderError::ResponseMalformed(
                "invalid media supplement receipt".into(),
            ));
        }
        Ok(MediaContinuationReceipt {
            command_id: admission.command_id,
            response_id: admission.response_id,
            metadata: admission.metadata,
        })
    }
}
async fn read_bounded<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T, ProviderError> {
    if response
        .content_length()
        .is_some_and(|length| length > RESPONSE_BOUND as u64)
    {
        return Err(ProviderError::ResponseMalformed(
            "media receipt exceeds bound".into(),
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
        if bytes.len() + chunk.len() > RESPONSE_BOUND {
            return Err(ProviderError::ResponseMalformed(
                "media receipt exceeds bound".into(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| ProviderError::ResponseMalformed("invalid media receipt".into()))
}
