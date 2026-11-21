use super::protocol::{sha256, MediaArtifactClaim};
use super::{MediaContinuations, MediaUploadRecord};
use layer1_public::rest::binding_media::BindingMediaError as Error;
use layer2_core::sandbox::agent_io::{
    AdmissionReceipt, AgentCommand, AgentCommandEnvelope, AgentContent, CommandAuthorizationError,
    CommandSourceGuard,
};
use serde::Deserialize;

impl MediaContinuations {
    pub(super) async fn attest(
        &self,
        parent: &AgentCommandEnvelope,
        generation: Option<u64>,
        expiry: i64,
        artifact: &MediaArtifactClaim,
    ) -> Result<(), Error> {
        self.validate_parent(parent)?;
        artifact.validate().map_err(|_| Error::Invalid)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| Error::Unavailable)?
            .as_secs();
        if expiry <= 0 || expiry as u64 <= now || generation == Some(0) {
            return Err(Error::Forbidden);
        }
        let event = parent.command.external_event().ok_or(Error::Forbidden)?;
        // The retained value was serialized by Gateway. Hash its complete wire
        // representation; Gateway checks that digest against its sealed Activity.
        // Manager consumes only attachment/identity fields, never normalizes it.
        let activity = event.payload.get("activity").ok_or(Error::Forbidden)?;
        let attachments = activity
            .pointer("/kind/attachments")
            .and_then(serde_json::Value::as_array)
            .ok_or(Error::Forbidden)?;
        if !matches!(
            activity.pointer("/kind/type").and_then(|v| v.as_str()),
            Some("message" | "event")
        ) || attachments
            .get(usize::from(artifact.attachment_index))
            .and_then(|a| a.get("source"))
            .is_none_or(|source| source.is_null())
            || activity.get("event_id").and_then(|v| v.as_str()) != Some(&event.event_id)
            || event.payload.get("channel_id").and_then(|v| v.as_str()) != Some(&event.source_id)
            || event
                .payload
                .get("conversation_id")
                .and_then(|v| v.as_str())
                != Some(&event.conversation_id)
            || event.payload.get("event_ref").and_then(|v| v.as_str()) != Some(&event.reference)
        {
            return Err(Error::Forbidden);
        }
        let body = serde_json::json!({"event_ref":event.reference,
            "provider_binding_id":parent.authorization.authority_id.strip_prefix("binding:").ok_or(Error::Forbidden)?,
            "channel_id":event.source_id,"conversation_id":event.conversation_id,"event_id":event.event_id,
            "activity_sha256":super::protocol::activity_digest(activity.clone()),
            "binding_generation":generation,"event_expires_at":expiry,"artifact":artifact});
        if serde_json::to_vec(&body).map_err(|_| Error::Invalid)?.len() > 16_384 {
            return Err(Error::Invalid);
        }
        let mut response = self
            .client
            .post(self.gateway.clone())
            .json(&body)
            .send()
            .await
            .map_err(|_| Error::Unavailable)?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(Error::Unavailable);
        }
        if response.content_length().is_some_and(|v| v > 16_384) {
            return Err(Error::Unavailable);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Unavailable)? {
            if bytes.len() + chunk.len() > 16_384 {
                return Err(Error::Unavailable);
            }
            bytes.extend_from_slice(&chunk);
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Attestation {
            current: bool,
        }
        let result: Attestation = serde_json::from_slice(&bytes).map_err(|_| Error::Unavailable)?;
        if !result.current {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
    pub(super) async fn check_upload(
        &self,
        record: &MediaUploadRecord,
        artifact: &MediaArtifactClaim,
    ) -> Result<AdmissionReceipt, Error> {
        record.validate()?;
        if artifact.original() != record.artifact {
            return Err(Error::Conflict);
        }
        let parent = self
            .store
            .get_command(&record.parent_command_id)
            .await
            .map_err(|_| Error::Unavailable)?
            .ok_or(Error::Forbidden)?;
        parent.validate().map_err(|_| Error::Unavailable)?;
        let receipt = parent.receipt;
        let parent = &receipt.envelope;
        self.validate_parent(parent)?;
        if parent.request_fingerprint != record.parent_fingerprint
            || parent.agent_id != record.agent_id
            || parent.authorization.grant_revision != Some(record.binding_generation)
        {
            return Err(Error::Forbidden);
        }
        let authority = self
            .authority
            .authorize_resource_command(&self.access, parent)
            .await
            .map_err(|error| match error {
                layer2_core::sandbox::agent_io::ResourceCommandAuthorizationError::Custody(
                    CommandAuthorizationError::Unavailable,
                ) => Error::Unavailable,
                layer2_core::sandbox::agent_io::ResourceCommandAuthorizationError::Resource(
                    error,
                ) => super::parent::resource_error(error),
                _ => Error::Forbidden,
            })?;
        authority
            .current_snapshot()
            .await
            .map_err(super::parent::resource_error)?;
        self.attest(
            parent,
            record.source_generation,
            record.source_expires_at,
            artifact,
        )
        .await?;
        Ok(receipt)
    }
}
#[async_trait::async_trait]
impl CommandSourceGuard for MediaContinuations {
    async fn check(
        &self,
        envelope: &AgentCommandEnvelope,
    ) -> Result<(), CommandAuthorizationError> {
        self.check_continuation(envelope)
            .await
            .map_err(|error| match error {
                Error::Unavailable | Error::NotReady => CommandAuthorizationError::Unavailable,
                _ => CommandAuthorizationError::Revoked,
            })
    }
}
impl MediaContinuations {
    async fn check_continuation(&self, envelope: &AgentCommandEnvelope) -> Result<(), Error> {
        let Some(proof) = &envelope.continuation else {
            return Ok(());
        };
        proof.validate().map_err(|_| Error::Forbidden)?;
        let artifact: MediaArtifactClaim =
            serde_json::from_value(proof.source_proof.clone()).map_err(|_| Error::Forbidden)?;
        if proof.artifact_id != format!("{}:{}", artifact.attachment_index, artifact.kind.as_str())
        {
            return Err(Error::Forbidden);
        }
        let record = self
            .uploads
            .get_media_upload(&MediaUploadRecord::id(
                &proof.parent_command_id,
                artifact.attachment_index,
            ))
            .await?
            .ok_or(Error::Forbidden)?;
        if record.receipt.is_none()
            || record.binding_generation != proof.binding_generation
            || record.source_generation != proof.source_generation
            || record.source_expires_at != proof.source_expires_at
        {
            return Err(Error::Forbidden);
        }
        let text = match &envelope.command {
            AgentCommand::Message { input, .. } => {
                let mut text = String::new();
                for item in &input.content {
                    let AgentContent::Text { text: part } = item else {
                        return Err(Error::Forbidden);
                    };
                    text.push_str(part);
                }
                text
            }
            _ => return Err(Error::Forbidden),
        };
        if artifact.text_sha256.as_deref() != Some(sha256(text.as_bytes()).as_str()) {
            return Err(Error::Forbidden);
        }
        let parent = self.check_upload(&record, &artifact).await?;
        proof
            .require_parent(envelope, &parent)
            .map_err(|_| Error::Forbidden)?;
        Ok(())
    }
}
