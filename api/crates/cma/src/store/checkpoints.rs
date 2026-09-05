//! Internal schema-v1 checkpoint persistence.
//!
//! Layer 2 exposes checkpoint identity and retention, but not the final
//! lifecycle store port. This adapter keeps the atomic seal boundary private
//! until the sandbox orchestration contract is integrated.

use layer2_core::ids::SandboxId;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Sandbox, SandboxCheckpointRef};
use mongodb::bson::{doc, Document};
use mongodb::Collection;
use serde::{Deserialize, Serialize};

use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, signed, start_transaction, CommitOutcome,
    MongoSandboxStore, DOCUMENT_SCHEMA_VERSION, TRANSACTION_ATTEMPTS,
};
use super::sandboxes::{read_sandbox, sandbox_replacement_fields};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CheckpointDoc {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i64,
    sandbox_id: String,
    sandbox_revision: i64,
    sealed_at: String,
    record: SandboxCheckpointRef,
}

pub(super) async fn ensure_indexes(
    _collection: &Collection<Document>,
) -> Result<(), layer2_core::StoreError> {
    // `_id` is the only checkpoint index in #804. Explicit collection creation
    // in sandbox_mongo.rs makes this no-op intentional rather than lazy DDL.
    Ok(())
}

fn checkpoint_id(sandbox_id: &SandboxId, revision: u64) -> String {
    format!("{}:{revision:020}", sandbox_id.as_str())
}

fn checkpoint_document(
    sandbox_id: &SandboxId,
    sandbox_revision: u64,
    checkpoint: &SandboxCheckpointRef,
) -> Result<Document, SandboxStoreError> {
    mongodb::bson::serialize_to_document(&CheckpointDoc {
        id: checkpoint_id(sandbox_id, sandbox_revision),
        document_schema_version: DOCUMENT_SCHEMA_VERSION,
        sandbox_id: sandbox_id.0.clone(),
        sandbox_revision: signed(sandbox_revision, "checkpoint sandbox revision")
            .map_err(SandboxStoreError::Corrupt)?,
        sealed_at: checkpoint.sealed_at.clone(),
        record: checkpoint.clone(),
    })
    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
}

fn read_checkpoint(document: Document) -> Result<CheckpointDoc, SandboxStoreError> {
    let checkpoint: CheckpointDoc = mongodb::bson::deserialize_from_document(document)
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    if checkpoint.document_schema_version != DOCUMENT_SCHEMA_VERSION {
        return Err(SandboxStoreError::Corrupt(format!(
            "unsupported checkpoint schema version {}",
            checkpoint.document_schema_version
        )));
    }
    Ok(checkpoint)
}

enum SealError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

impl MongoSandboxStore {
    /// Atomically retain a sealed checkpoint and advance the sandbox CAS.
    ///
    /// This remains crate-private because Layer 2 has not named the checkpoint
    /// command yet; callers cannot accidentally depend on this provisional
    /// adapter shape.
    pub(crate) async fn seal_checkpoint(
        &self,
        expected_revision: u64,
        sandbox: Sandbox,
    ) -> Result<(), SandboxStoreError> {
        if sandbox.revision
            != expected_revision
                .checked_add(1)
                .ok_or(SandboxStoreError::Conflict)?
        {
            return Err(SandboxStoreError::Conflict);
        }
        let checkpoint =
            sandbox
                .checkpoint
                .as_ref()
                .ok_or(SandboxStoreError::InvalidTransaction(
                    "sealed sandbox has no checkpoint reference",
                ))?;
        let checkpoint_document = checkpoint_document(&sandbox.id, sandbox.revision, checkpoint)?;
        let mut last_transient = None;
        for attempt in 0..TRANSACTION_ATTEMPTS {
            if attempt > 0 {
                tokio::task::yield_now().await;
            }
            let mut session = match self.client.start_session().await {
                Ok(session) => session,
                Err(error) if is_transient(&error) => {
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(error) => return Err(SandboxStoreError::Unavailable(error.to_string())),
            };
            if let Err(error) = start_transaction(&mut session).await {
                if is_transient(&error) {
                    last_transient = Some(error.to_string());
                    continue;
                }
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
            let result = async {
                self.checkpoints
                    .insert_one(checkpoint_document.clone())
                    .session(&mut session)
                    .await
                    .map_err(|error| {
                        if is_duplicate_key(&error) {
                            SealError::Domain(SandboxStoreError::Duplicate)
                        } else {
                            SealError::Driver(error)
                        }
                    })?;
                let updated = self
                    .sandboxes
                    .find_one_and_update(
                        doc! {
                            "_id": sandbox.id.as_str(),
                            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                            "revision": signed(expected_revision, "expected sandbox revision")
                                .map_err(SandboxStoreError::Corrupt)
                                .map_err(SealError::Domain)?,
                        },
                        doc! { "$set": sandbox_replacement_fields(&sandbox)
                        .map_err(SealError::Domain)? },
                    )
                    .session(&mut session)
                    .await
                    .map_err(SealError::Driver)?;
                if updated.is_none() {
                    return Err(SealError::Domain(SandboxStoreError::Conflict));
                }
                Ok::<(), SealError>(())
            }
            .await;
            match result {
                Ok(()) => {}
                Err(SealError::Domain(error)) => {
                    let _ = session.abort_transaction().await;
                    if error == SandboxStoreError::Duplicate
                        && self.reconcile_checkpoint(&sandbox).await?
                    {
                        return Ok(());
                    }
                    return Err(error);
                }
                Err(SealError::Driver(error)) if is_transient(&error) => {
                    let _ = session.abort_transaction().await;
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(SealError::Driver(error)) => {
                    let _ = session.abort_transaction().await;
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(()),
                CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
                CommitOutcome::Unknown(error) => {
                    if self.reconcile_checkpoint(&sandbox).await? {
                        return Ok(());
                    }
                    return Err(SandboxStoreError::Unavailable(format!(
                        "checkpoint seal commit remained unknown: {error}"
                    )));
                }
                CommitOutcome::Failed(error) => {
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            }
        }
        Err(SandboxStoreError::Unavailable(format!(
            "checkpoint seal exhausted its retry budget: {}",
            last_transient.unwrap_or_else(|| "no driver result".into())
        )))
    }

    async fn reconcile_checkpoint(&self, sandbox: &Sandbox) -> Result<bool, SandboxStoreError> {
        let stored_sandbox = self
            .sandboxes
            .find_one(doc! { "_id": sandbox.id.as_str() })
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
            .as_ref()
            .map(read_sandbox)
            .transpose()?;
        let stored_checkpoint = self
            .checkpoints
            .find_one(doc! { "_id": checkpoint_id(&sandbox.id, sandbox.revision) })
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
            .map(read_checkpoint)
            .transpose()?;
        Ok(stored_sandbox.as_ref() == Some(sandbox)
            && stored_checkpoint
                .as_ref()
                .is_some_and(|stored| sandbox.checkpoint.as_ref() == Some(&stored.record)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::ids::AgentId;
    use layer2_core::sandbox::SandboxCheckpointConversation;
    use layer3_adapter::{CheckpointReceipt, ConversationSnapshot, WriterState};
    use sha2::{Digest, Sha256};

    #[test]
    fn checkpoint_documents_are_explicitly_schema_v1() {
        let id = SandboxId("sbx_checkpoint".into());
        let archive = b"conversation snapshot".to_vec();
        let conversation = SandboxCheckpointConversation::new(
            AgentId("agt_checkpoint".into()),
            "conversation-checkpoint",
            ConversationSnapshot {
                conversation_id: "conversation-checkpoint".into(),
                archive_sha256: format!("{:x}", Sha256::digest(&archive)),
                archive,
            },
        )
        .unwrap();
        let checkpoint = SandboxCheckpointRef::seal(
            &CheckpointReceipt {
                epoch: 1,
                object_reference: "objects/checkpoint".into(),
                sha256: "a".repeat(64),
                writers: WriterState::Resumed,
            },
            &[conversation],
            "2026-09-05T00:00:00Z",
        )
        .unwrap();
        let document = checkpoint_document(&id, 4, &checkpoint).unwrap();
        assert_eq!(document.get_i64("document_schema_version").unwrap(), 1);
        assert_eq!(document.get_str("sandbox_id").unwrap(), id.as_str());
        assert_eq!(document.get_i64("sandbox_revision").unwrap(), 4);
    }
}
