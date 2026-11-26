//! Owner-authorized logical deletion retains a compact, immutable receipt.
use super::{service::*, *};
use crate::Caller;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentDeletionReceipt {
    pub id: String,
    pub revision: u64,
    pub deleted_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentTombstone {
    pub owner_hash: String,
    pub request_hash: String,
    pub expected_revision: u64,
    pub receipt: DocumentDeletionReceipt,
}

impl DocumentTombstone {
    pub fn validate(&self) -> Result<(), DocumentError> {
        if !valid_id(&self.receipt.id, "doc_")
            || !valid_digest(&self.owner_hash)
            || !valid_digest(&self.request_hash)
            || self.expected_revision == 0
            || self.expected_revision.checked_add(1) != Some(self.receipt.revision)
            || crate::seconds_from_rfc3339(&self.receipt.deleted_at).is_none()
        {
            return Err(DocumentError::Corrupt);
        }
        Ok(())
    }

    fn replay(
        &self,
        owner: &str,
        key: &str,
        revision: u64,
    ) -> Result<DocumentDeletionReceipt, DocumentError> {
        self.validate()?;
        if self.owner_hash != digest(owner.as_bytes()) {
            return Err(DocumentError::NotFound);
        }
        if self.request_hash != types::fingerprint(&(owner, "delete", key))? {
            return Err(DocumentError::NotFound);
        }
        if self.expected_revision != revision {
            return Err(DocumentError::IdempotencyConflict);
        }
        Ok(self.receipt.clone())
    }
}

impl DocumentService {
    pub fn with_deletion_enabled(mut self: Arc<Self>, enabled: bool) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("document deletion is configured before sharing")
            .deletion_enabled = enabled;
        self
    }

    pub async fn delete(
        &self,
        caller: &Caller,
        id: &str,
        key: &str,
        expected_revision: u64,
    ) -> Result<DocumentDeletionReceipt, DocumentError> {
        principal(caller)?;
        operation_key(key)?;
        self.require_account_available(&caller.subject).await?;
        if !valid_id(id, "doc_") {
            return Err(DocumentError::NotFound);
        }
        let owner = caller.subject.as_str();
        if let Some(tombstone) = self.store.tombstone(id).await? {
            return tombstone.replay(owner, key, expected_revision);
        }
        if !self.deletion_enabled {
            return Err(DocumentError::Unavailable);
        }
        let record = match self.owner(caller, id).await {
            Ok(record) => record,
            Err(DocumentError::NotFound) => {
                return self
                    .store
                    .tombstone(id)
                    .await?
                    .ok_or(DocumentError::NotFound)?
                    .replay(owner, key, expected_revision);
            }
            Err(error) => return Err(error),
        };
        if record.metadata.revision != expected_revision {
            return Err(DocumentError::Conflict);
        }
        let tombstone = DocumentTombstone {
            owner_hash: digest(owner.as_bytes()),
            request_hash: types::fingerprint(&(owner, "delete", key))?,
            expected_revision,
            receipt: DocumentDeletionReceipt {
                id: id.to_owned(),
                revision: expected_revision
                    .checked_add(1)
                    .ok_or(DocumentError::Capacity)?,
                deleted_at: crate::now_rfc3339(),
            },
        };
        tombstone.validate()?;
        if self.store.delete(owner, &tombstone).await? {
            return Ok(tombstone.receipt);
        }
        match self.store.tombstone(id).await? {
            Some(existing) => existing.replay(owner, key, expected_revision),
            None => Err(DocumentError::Conflict),
        }
    }
}
