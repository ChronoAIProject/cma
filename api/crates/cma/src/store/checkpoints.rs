//! Atomic schema-v1 checkpoint metadata publication.

use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::ids::SandboxId;
use layer2_core::sandbox::store::{SandboxCheckpointMutation, SandboxStoreError};
use layer2_core::sandbox::{
    Sandbox, SandboxCheckpointHistory, SandboxCheckpointRef, SandboxEvent, SandboxEventKind,
};
use mongodb::bson::{doc, Document};
use mongodb::options::IndexOptions;
use mongodb::{Collection, IndexModel};
use serde::{Deserialize, Serialize};

use super::checkpoint_objects::{prune_evicted, RetentionError};
use super::lifecycle_execution::LifecycleLoadError;
use super::sandbox_events::{append_in_transaction, TransactionEventError};
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, signed, start_transaction,
    wait_for_reconciliation, CommitOutcome, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
    TRANSACTION_ATTEMPTS,
};
use super::sandboxes::{read_sandbox, sandbox_replacement_fields};

mod agents;
mod reconciliation;
mod transaction;
use reconciliation::{reconcile_after_unknown, reconcile_publication};

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
    collection: &Collection<Document>,
) -> Result<(), layer2_core::StoreError> {
    collection
        .create_indexes(checkpoint_indexes())
        .await
        .map(|_| ())
        .map_err(|error| {
            layer2_core::StoreError::Unavailable(format!("checkpoint indexes: {error}"))
        })
}

fn checkpoint_indexes() -> Vec<IndexModel> {
    vec![IndexModel::builder()
        .keys(doc! { "sandbox_id": 1, "sandbox_revision": -1 })
        .options(
            IndexOptions::builder()
                .name("checkpoints_sandbox_revision".to_string())
                .build(),
        )
        .build()]
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

enum PublishError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

impl From<LifecycleLoadError> for PublishError {
    fn from(error: LifecycleLoadError) -> Self {
        match error {
            LifecycleLoadError::Domain(error) => Self::Domain(error),
            LifecycleLoadError::Driver(error) => Self::Driver(error),
        }
    }
}

impl From<TransactionEventError> for PublishError {
    fn from(error: TransactionEventError) -> Self {
        match error {
            TransactionEventError::Domain(error) => Self::Domain(error),
            TransactionEventError::Driver(error) => Self::Driver(error),
        }
    }
}

impl From<RetentionError> for PublishError {
    fn from(error: RetentionError) -> Self {
        match error {
            RetentionError::Corrupt(detail) => {
                Self::Domain(SandboxStoreError::Corrupt(detail.into()))
            }
            RetentionError::Driver(error) => Self::Driver(error),
        }
    }
}

struct PublicationReceipt {
    sandbox: Sandbox,
    checkpoint: SandboxCheckpointRef,
    event: SandboxEvent,
}

pub(super) async fn publish_checkpoint(
    store: &MongoSandboxStore,
    mutation: SandboxCheckpointMutation,
) -> Result<(), SandboxStoreError> {
    let mut last_transient = None;
    for attempt in 0..TRANSACTION_ATTEMPTS {
        if attempt > 0 {
            tokio::task::yield_now().await;
        }
        let mut session = match store.client.start_session().await {
            Ok(session) => session,
            Err(error) if is_transient(&error) => {
                last_transient = Some(error.to_string());
                continue;
            }
            Err(error) => return Err(SandboxStoreError::Unavailable(error.to_string())),
        };
        let receipt = match transaction::attempt(store, &mutation, &mut session).await {
            Ok(receipt) => receipt,
            Err(PublishError::Domain(error)) => {
                let _ = session.abort_guarded().await;
                if (error == SandboxStoreError::Duplicate
                    || (mutation.nonblocking && error == SandboxStoreError::Conflict))
                    && reconcile_publication(store, &mutation).await?
                {
                    return Ok(());
                }
                return Err(error);
            }
            Err(PublishError::Driver(error)) if is_transient(&error) => {
                let _ = session.abort_guarded().await;
                last_transient = Some(error.to_string());
                continue;
            }
            Err(PublishError::Driver(error)) => {
                let _ = session.abort_guarded().await;
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(()),
            CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
            CommitOutcome::Unknown(error) => {
                let snapshot_time = session.operation_time().ok_or_else(|| {
                    SandboxStoreError::Unavailable(format!(
                        "checkpoint publication commit remained unknown without an operation time: {error}"
                    ))
                })?;
                let commit_error = error.to_string();
                let unresolved = commit_error.clone();
                let store = store.clone();
                return wait_for_reconciliation("checkpoint publication", async move {
                    if reconcile_after_unknown(&store, &receipt, snapshot_time).await? {
                        Ok(())
                    } else {
                        Err(SandboxStoreError::Unavailable(format!(
                            "checkpoint publication commit remained unknown: {unresolved}"
                        )))
                    }
                })
                .await
                .map_err(|deadline| {
                    SandboxStoreError::Unavailable(format!(
                        "{deadline}; ambiguous commit: {commit_error}"
                    ))
                })?;
            }
            CommitOutcome::Failed(error) => {
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
        }
    }
    Err(SandboxStoreError::Unavailable(format!(
        "checkpoint publication exhausted its retry budget: {}",
        last_transient.unwrap_or_else(|| "no driver result".into())
    )))
}

fn evicted_archives<'a>(
    current: &'a SandboxCheckpointHistory,
    retained: &SandboxCheckpointHistory,
) -> Vec<&'a SandboxCheckpointRef> {
    // Other unreferenced manifests may still be awaiting publication. Only the
    // completed generation displaced by this validated transition is deletable.
    current
        .previous
        .iter()
        .chain(current.latest.iter())
        .filter(|evicted| {
            retained
                .latest
                .iter()
                .chain(retained.previous.iter())
                .all(|checkpoint| checkpoint.object_reference != evicted.object_reference)
        })
        .collect()
}

async fn prune_checkpoint_metadata(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    session: &mut mongodb::ClientSession,
) -> Result<(), PublishError> {
    let retained = sandbox
        .lifecycle
        .checkpoints
        .latest
        .iter()
        .chain(sandbox.lifecycle.checkpoints.previous.iter())
        .map(|checkpoint| {
            signed(checkpoint.epoch, "retained checkpoint epoch")
                .map_err(SandboxStoreError::Corrupt)
                .map_err(PublishError::Domain)
        })
        .collect::<Result<Vec<_>, _>>()?;
    store
        .checkpoints
        .delete_many(doc! {
            "sandbox_id": sandbox.id.as_str(),
            "record.epoch": { "$nin": retained },
        })
        .session(session)
        .await
        .map(|_| ())
        .map_err(PublishError::Driver)
}

fn classify_write(error: mongodb::error::Error) -> PublishError {
    if is_duplicate_key(&error) {
        PublishError::Domain(SandboxStoreError::Duplicate)
    } else {
        PublishError::Driver(error)
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
                writers: WriterState::Held,
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

    #[test]
    fn checkpoint_index_supports_retention_and_purge_by_sandbox() {
        let index = checkpoint_indexes().remove(0);
        assert_eq!(index.keys, doc! { "sandbox_id": 1, "sandbox_revision": -1 });
        assert_eq!(
            index.options.unwrap().name.as_deref(),
            Some("checkpoints_sandbox_revision")
        );
    }

    #[test]
    fn retention_selects_only_an_evicted_unshared_archive() {
        let first = checkpoint(1, "home-first.cma", "a");
        let second = checkpoint(2, "home-second.cma", "b");
        let third = checkpoint(3, "home-third.cma", "c");
        let current = SandboxCheckpointHistory {
            latest: Some(second.clone()),
            previous: Some(first.clone()),
        };
        let retained = SandboxCheckpointHistory {
            latest: Some(third),
            previous: Some(second),
        };
        assert_eq!(evicted_archives(&current, &retained), vec![&first]);

        let shared = SandboxCheckpointHistory {
            latest: retained.latest,
            previous: Some(first.clone()),
        };
        assert_eq!(
            evicted_archives(&current, &shared),
            vec![current.latest.as_ref().unwrap()]
        );

        let latest_only = SandboxCheckpointHistory {
            latest: shared.latest,
            previous: None,
        };
        assert_eq!(
            evicted_archives(&current, &latest_only),
            vec![&first, current.latest.as_ref().unwrap()]
        );
    }

    fn checkpoint(epoch: u64, object_reference: &str, digest_digit: &str) -> SandboxCheckpointRef {
        SandboxCheckpointRef::seal(
            &CheckpointReceipt {
                epoch,
                object_reference: object_reference.into(),
                sha256: digest_digit.repeat(64),
                writers: WriterState::Held,
            },
            &[],
            "2026-09-05T00:00:00Z",
        )
        .unwrap()
    }
}
