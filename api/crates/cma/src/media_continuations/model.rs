use super::protocol::{sha256, MediaArtifactClaim, MediaContinuationKind};
use layer1_public::rest::binding_media::BindingMediaError;
use layer2_core::{ids::AgentCommandId, sandbox::agent_attachments::AgentAttachment, AgentId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MediaUploadRecord {
    pub id: String,
    pub parent_command_id: AgentCommandId,
    pub parent_fingerprint: String,
    pub agent_id: AgentId,
    pub binding_generation: u64,
    pub source_generation: Option<u64>,
    pub source_expires_at: i64,
    pub artifact: MediaArtifactClaim,
    pub receipt: Option<AgentAttachment>,
}
impl MediaUploadRecord {
    pub fn id(parent: &AgentCommandId, index: u8) -> String {
        format!(
            "att_{}",
            sha256(
                &serde_json::to_vec(&("cma-event-media-v1", parent, index))
                    .expect("typed identity")
            )
        )
    }
    pub fn validate(&self) -> Result<(), BindingMediaError> {
        self.artifact
            .validate()
            .map_err(|_| BindingMediaError::Conflict)?;
        if self.id != Self::id(&self.parent_command_id, self.artifact.attachment_index)
            || self.artifact.kind != MediaContinuationKind::Original
            || self.artifact.text_sha256.is_some()
            || self.parent_fingerprint.len() != 64
            || self.binding_generation == 0
            || self.source_generation == Some(0)
            || self.source_expires_at <= 0
            || self
                .receipt
                .as_ref()
                .is_some_and(|r| r != &self.expected_receipt())
        {
            return Err(BindingMediaError::Conflict);
        }
        Ok(())
    }
    pub fn expected_receipt(&self) -> AgentAttachment {
        AgentAttachment {
            id: self.id.clone(),
            agent_id: self.agent_id.clone(),
            name: self.artifact.name.clone(),
            path: format!(
                "/home/attachments/{}/{}/{}",
                self.agent_id, self.id, self.artifact.name
            ),
            size_bytes: self.artifact.size_bytes,
            media_type: self.artifact.media_type.clone(),
            sha256: self.artifact.source_sha256.clone(),
        }
    }
    pub fn same_intent(&self, other: &Self) -> bool {
        Self {
            receipt: None,
            ..self.clone()
        } == Self {
            receipt: None,
            ..other.clone()
        }
    }
}
#[async_trait::async_trait]
pub(crate) trait MediaUploadStore: Send + Sync {
    async fn get_media_upload(
        &self,
        id: &str,
    ) -> Result<Option<MediaUploadRecord>, BindingMediaError>;
    async fn reserve_media_upload(
        &self,
        record: &MediaUploadRecord,
    ) -> Result<MediaUploadRecord, BindingMediaError>;
    async fn complete_media_upload(
        &self,
        record: &MediaUploadRecord,
        receipt: &AgentAttachment,
    ) -> Result<MediaUploadRecord, BindingMediaError>;
}
