//! Permanent exact-identity receipts for binding-event uploads. No file blobs.
use super::sandbox_mongo::{
    is_duplicate_key, read_record, record_bson, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};
use crate::media_continuations::{MediaUploadRecord, MediaUploadStore};
use layer1_public::rest::binding_media::BindingMediaError as Error;
use layer2_core::sandbox::agent_attachments::AgentAttachment;
use mongodb::bson::{doc, Document};
pub(super) const COLLECTION: &str = "binding_event_media_uploads";
fn read(row: &Document) -> Result<MediaUploadRecord, Error> {
    let record: MediaUploadRecord = read_record(row).map_err(|_| Error::Unavailable)?;
    record.validate()?;
    if row.get_str("_id").ok() != Some(&record.id) {
        return Err(Error::Unavailable);
    }
    Ok(record)
}

pub(in crate::store) async fn require_continuation(
    store: &MongoSandboxStore,
    envelope: &layer2_core::sandbox::agent_io::AgentCommandEnvelope,
    session: &mut mongodb::ClientSession,
) -> Result<(), super::agent_commands::ApplyError> {
    use layer2_core::sandbox::agent_io::CommandLedgerError;
    let Some(proof) = &envelope.continuation else {
        return Ok(());
    };
    let parent = store
        .agent_commands
        .find_one(doc! {"_id":proof.parent_command_id.as_str()})
        .session(&mut *session)
        .await?
        .ok_or(CommandLedgerError::NotFound)?;
    proof.require_parent(
        envelope,
        &super::agent_commands::read_command(&parent)?.receipt,
    )?;
    let artifact: crate::media_continuations::protocol::MediaArtifactClaim =
        serde_json::from_value(proof.source_proof.clone())
            .map_err(|_| CommandLedgerError::Invalid)?;
    let id = MediaUploadRecord::id(&proof.parent_command_id, artifact.attachment_index);
    let row = store
        .media_uploads
        .find_one(doc! {"_id":id})
        .session(session)
        .await?
        .ok_or(CommandLedgerError::NotFound)?;
    let record = read(&row).map_err(|_| CommandLedgerError::Corrupt)?;
    if record.receipt.is_none()
        || record.artifact != artifact.original()
        || record.parent_fingerprint != proof.parent_fingerprint
        || record.agent_id != envelope.agent_id
        || record.binding_generation != proof.binding_generation
        || record.source_generation != proof.source_generation
        || record.source_expires_at != proof.source_expires_at
        || proof.artifact_id != format!("{}:{}", artifact.attachment_index, artifact.kind.as_str())
    {
        return Err(CommandLedgerError::Conflict.into());
    }
    Ok(())
}
#[async_trait::async_trait]
impl MediaUploadStore for MongoSandboxStore {
    async fn get_media_upload(&self, id: &str) -> Result<Option<MediaUploadRecord>, Error> {
        self.media_uploads
            .find_one(doc! {"_id":id})
            .await
            .map_err(|_| Error::Unavailable)?
            .as_ref()
            .map(read)
            .transpose()
    }
    async fn reserve_media_upload(
        &self,
        record: &MediaUploadRecord,
    ) -> Result<MediaUploadRecord, Error> {
        record.validate()?;
        if record.receipt.is_some() {
            return Err(Error::Invalid);
        }
        if let Some(current) = self.get_media_upload(&record.id).await? {
            return if current.same_intent(record) {
                Ok(current)
            } else {
                Err(Error::Conflict)
            };
        }
        let document = doc! {"_id":&record.id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "record":record_bson(record).map_err(|_|Error::Invalid)?};
        match self.media_uploads.insert_one(document).await {
            Ok(_) => Ok(record.clone()),
            Err(error) if is_duplicate_key(&error) => {
                let current = self
                    .get_media_upload(&record.id)
                    .await?
                    .ok_or(Error::Unavailable)?;
                if current.same_intent(record) {
                    Ok(current)
                } else {
                    Err(Error::Conflict)
                }
            }
            Err(_) => Err(Error::Unavailable),
        }
    }
    async fn complete_media_upload(
        &self,
        record: &MediaUploadRecord,
        receipt: &AgentAttachment,
    ) -> Result<MediaUploadRecord, Error> {
        record.validate()?;
        if receipt != &record.expected_receipt() {
            return Err(Error::Conflict);
        }
        let pending = MediaUploadRecord {
            receipt: None,
            ..record.clone()
        };
        let completed = MediaUploadRecord {
            receipt: Some(receipt.clone()),
            ..record.clone()
        };
        self.media_uploads
            .update_one(
                doc! {"_id":&record.id,"record":record_bson(&pending).map_err(|_|Error::Invalid)?},
                doc! {"$set":{"record":record_bson(&completed).map_err(|_|Error::Invalid)?}},
            )
            .await
            .map_err(|_| Error::Unavailable)?;
        let current = self
            .get_media_upload(&record.id)
            .await?
            .ok_or(Error::Unavailable)?;
        if current == completed {
            Ok(current)
        } else {
            Err(Error::Conflict)
        }
    }
}
