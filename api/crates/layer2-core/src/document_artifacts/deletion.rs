//! Logical deletion retains only the evidence needed to fence retries and purge.
use super::{service::*, *};
use crate::Caller;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteDocument {
    pub expected_revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentPurgeState {
    Pending,
    Complete,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentDeletion {
    pub id: String,
    pub revision: u64,
    pub deleted_at: String,
    pub purge: DocumentPurgeState,
}

/// Permanent identity fence; contains no live policy or document content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentTombstone {
    pub id: String,
    pub owner_hash: String,
    pub revision: u64,
    pub deleted_at: String,
    pub receipt_key: String,
    pub fingerprint: String,
    pub objects: Vec<String>,
}

impl DocumentTombstone {
    pub fn validate(&self) -> Result<(), DocumentError> {
        let prefix = format!("documents/{}/{}/", self.owner_hash, self.id);
        if !valid_id(&self.id, "doc_")
            || !valid_digest(&self.owner_hash)
            || self.revision < 2
            || self.deleted_at.parse::<jiff::Timestamp>().is_err()
            || !valid_digest(&self.receipt_key)
            || !valid_digest(&self.fingerprint)
            || self.objects.len() > MAX_REVISIONS
            || self
                .objects
                .iter()
                .any(|key| !valid_purge_object(&prefix, key))
        {
            return Err(DocumentError::Corrupt);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteOutcome {
    Deleted,
    Conflict,
    NotFound,
    AlreadyDeleted(DocumentTombstone),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertOutcome {
    Inserted,
    Exists,
    Deleted,
}

impl DocumentService {
    pub async fn delete(
        &self,
        caller: &Caller,
        id: &str,
        key: &str,
        body: DeleteDocument,
    ) -> Result<DocumentDeletion, DocumentError> {
        // Feature refusal must not inspect or mutate lifecycle persistence.
        if !self.deletion_enabled {
            return Err(DocumentError::DeletionUnavailable);
        }
        principal(caller)?;
        operation_key(key)?;
        if !valid_id(id, "doc_") {
            return Err(DocumentError::NotFound);
        }
        if body.expected_revision == 0 {
            return Err(DocumentError::Invalid);
        }
        let receipt_key = types::fingerprint(&(caller.subject.as_str(), "delete", key))?;
        let fingerprint = types::fingerprint(&body)?;
        let Some(record) = self.store.get(id).await? else {
            let tombstone = self
                .store
                .tombstone(id)
                .await?
                .ok_or(DocumentError::NotFound)?;
            return self
                .deletion_replay(caller, &receipt_key, &fingerprint, tombstone)
                .await;
        };
        if record.metadata.owner_subject != caller.subject.as_str() {
            return Err(DocumentError::NotFound);
        }
        if record.metadata.revision != body.expected_revision {
            return Err(DocumentError::Conflict);
        }
        let tombstone = DocumentTombstone {
            id: id.into(),
            owner_hash: digest(caller.subject.as_str().as_bytes()),
            revision: body
                .expected_revision
                .checked_add(1)
                .ok_or(DocumentError::Capacity)?,
            deleted_at: crate::now_rfc3339(),
            receipt_key: receipt_key.clone(),
            fingerprint: fingerprint.clone(),
            objects: record
                .revisions
                .into_iter()
                .map(|revision| revision.object_key)
                .collect(),
        };
        tombstone.validate()?;
        match self
            .store
            .delete(body.expected_revision, &tombstone)
            .await?
        {
            DeleteOutcome::Deleted => self.deletion_result(tombstone).await,
            DeleteOutcome::AlreadyDeleted(previous) => {
                self.deletion_replay(caller, &receipt_key, &fingerprint, previous)
                    .await
            }
            DeleteOutcome::Conflict => Err(DocumentError::Conflict),
            DeleteOutcome::NotFound => Err(DocumentError::NotFound),
        }
    }

    async fn deletion_replay(
        &self,
        caller: &Caller,
        receipt_key: &str,
        fingerprint: &str,
        tombstone: DocumentTombstone,
    ) -> Result<DocumentDeletion, DocumentError> {
        tombstone.validate()?;
        if tombstone.owner_hash != digest(caller.subject.as_str().as_bytes())
            || tombstone.receipt_key != receipt_key
        {
            return Err(DocumentError::NotFound);
        }
        if tombstone.fingerprint != fingerprint {
            return Err(DocumentError::IdempotencyConflict);
        }
        self.deletion_result(tombstone).await
    }

    async fn deletion_result(
        &self,
        tombstone: DocumentTombstone,
    ) -> Result<DocumentDeletion, DocumentError> {
        let purge = self
            .store
            .purge_state(&tombstone.id)
            .await?
            .ok_or(DocumentError::Corrupt)?;
        Ok(DocumentDeletion {
            id: tombstone.id,
            revision: tombstone.revision,
            deleted_at: tombstone.deleted_at,
            purge,
        })
    }
}

pub(super) fn valid_purge_object(prefix: &str, key: &str) -> bool {
    key.strip_prefix(prefix)
        .and_then(|file| file.strip_suffix(".md"))
        .is_some_and(valid_digest)
}
