use super::protocol::{sha256, MediaArtifactClaim, MediaContinuationKind};
use super::{MediaContinuations, MediaUploadRecord};
use layer1_public::{
    rest::binding_media::{
        BindingMediaError as Error, BindingMediaPath, BindingMediaService, BindingMediaSupplement,
        BindingMediaUpload, BindingMediaUploadParams,
    },
    AuthenticatedResourceCaller,
};
use layer2_core::sandbox::{
    agent_attachments::{
        AgentAttachment, AttachmentAdmission, AttachmentError, AttachmentMetadata,
        AttachmentSourceGuard,
    },
    agent_io::AgentResponseReading,
};
use std::sync::Arc;

struct UploadProof {
    service: Arc<MediaContinuations>,
    record: MediaUploadRecord,
}
#[async_trait::async_trait]
impl AttachmentSourceGuard for UploadProof {
    async fn check(&self) -> Result<(), AttachmentError> {
        self.service
            .check_upload(&self.record, &self.record.artifact)
            .await
            .map(|_| ())
            .map_err(|e| match e {
                Error::Unavailable => AttachmentError::Unavailable,
                _ => AttachmentError::Conflict,
            })
    }
}
struct Upload {
    service: Arc<MediaContinuations>,
    record: MediaUploadRecord,
    admission: AttachmentAdmission,
}
#[async_trait::async_trait]
impl BindingMediaUpload for Upload {
    async fn upload(self: Box<Self>, bytes: Vec<u8>) -> Result<AgentAttachment, Error> {
        if bytes.len() as u64 != self.record.artifact.size_bytes
            || sha256(&bytes) != self.record.artifact.source_sha256
        {
            return Err(Error::Conflict);
        }
        self.service
            .check_upload(&self.record, &self.record.artifact)
            .await?;
        if let Some(receipt) = &self.record.receipt {
            return Ok(receipt.clone());
        }
        let receipt = self
            .service
            .attachments
            .upload(self.admission, bytes)
            .await
            .map_err(attachment_error)?;
        if receipt != self.record.expected_receipt() {
            return Err(Error::Conflict);
        }
        let saved = self
            .service
            .uploads
            .complete_media_upload(&self.record, &receipt)
            .await?;
        self.service.check_upload(&saved, &saved.artifact).await?;
        saved.receipt.ok_or(Error::Unavailable)
    }
}
#[async_trait::async_trait]
impl BindingMediaService for MediaContinuations {
    async fn attachment(
        &self,
        caller: AuthenticatedResourceCaller,
        path: BindingMediaPath,
    ) -> Result<AgentAttachment, Error> {
        let (parent, _) = self.parent_for_caller(&caller, &path).await?;
        let record = self
            .uploads
            .get_media_upload(&MediaUploadRecord::id(
                &parent.receipt.envelope.command_id,
                path.attachment_index,
            ))
            .await?
            .ok_or(Error::NotFound)?;
        self.check_upload(&record, &record.artifact).await?;
        record.receipt.ok_or(Error::NotFound)
    }

    async fn prepare_upload(
        self: Arc<Self>,
        caller: AuthenticatedResourceCaller,
        path: BindingMediaPath,
        params: BindingMediaUploadParams,
        media_type: String,
        size_bytes: u64,
    ) -> Result<Box<dyn BindingMediaUpload>, Error> {
        let (parent, authorized) = self.parent_for_caller(&caller, &path).await?;
        let parent = parent.receipt.envelope;
        let artifact = MediaArtifactClaim {
            attachment_index: path.attachment_index,
            kind: MediaContinuationKind::Original,
            content_sha256: params.sha256.clone(),
            source_sha256: params.sha256,
            name: params.name,
            media_type,
            size_bytes,
            text_sha256: None,
        };
        artifact.validate().map_err(|_| Error::Invalid)?;
        self.attest(
            &parent,
            params.gateway_binding_generation,
            params.event_expires_at,
            &artifact,
        )
        .await?;
        let record = MediaUploadRecord {
            id: MediaUploadRecord::id(&parent.command_id, path.attachment_index),
            parent_command_id: parent.command_id.clone(),
            parent_fingerprint: parent.request_fingerprint.clone(),
            agent_id: parent.agent_id.clone(),
            binding_generation: parent
                .authorization
                .grant_revision
                .ok_or(Error::Forbidden)?,
            source_generation: params.gateway_binding_generation,
            source_expires_at: params.event_expires_at,
            artifact,
            receipt: None,
        };
        let record = self.uploads.reserve_media_upload(&record).await?;
        let admission = self
            .attachments
            .admit_external_event(
                authorized,
                &parent,
                record.id.clone(),
                AttachmentMetadata {
                    key: &record.id,
                    name: &record.artifact.name,
                    media_type: &record.artifact.media_type,
                },
                Arc::new(UploadProof {
                    service: self.clone(),
                    record: record.clone(),
                }),
            )
            .map_err(attachment_error)?;
        Ok(Box::new(Upload {
            service: self,
            record,
            admission,
        }))
    }
    async fn supplement(
        &self,
        caller: AuthenticatedResourceCaller,
        path: BindingMediaPath,
        body: BindingMediaSupplement,
    ) -> Result<AgentResponseReading, Error> {
        self.admit_supplement(caller, path, body).await
    }
}
fn attachment_error(error: AttachmentError) -> Error {
    match error {
        AttachmentError::Invalid => Error::Invalid,
        AttachmentError::TooLarge => Error::TooLarge,
        AttachmentError::NotReady | AttachmentError::Unsupported => Error::NotReady,
        AttachmentError::Conflict => Error::Conflict,
        AttachmentError::Access(error) => super::parent::resource_error(error),
        AttachmentError::Unavailable => Error::Unavailable,
    }
}
