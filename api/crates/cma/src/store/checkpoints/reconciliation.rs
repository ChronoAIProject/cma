//! Checkpoint receipt verification from one committed snapshot.

use layer2_core::sandbox::store::{SandboxCheckpointMutation, SandboxStoreError};
use mongodb::bson::{doc, Timestamp};
use mongodb::options::ReadConcern;
use mongodb::ClientSession;

use super::super::sandbox_events::{read_event, reconcile_event_with_snapshot_session};
use super::super::sandbox_mongo::MongoSandboxStore;
use super::super::sandboxes::read_sandbox;
use super::{checkpoint_id, read_checkpoint, PublicationReceipt};

pub(super) async fn reconcile_after_unknown(
    store: &MongoSandboxStore,
    receipt: &PublicationReceipt,
    snapshot_time: Timestamp,
) -> Result<bool, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(snapshot_time)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    reconcile_receipt(store, receipt, &mut session).await
}

pub(super) async fn reconcile_publication(
    store: &MongoSandboxStore,
    mutation: &SandboxCheckpointMutation,
) -> Result<bool, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let event = store
        .sandbox_events
        .find_one(doc! {
            "sandbox_id": mutation.sandbox.id.as_str(),
            "record.kind.type": "checkpoint_sealed",
            "record.kind.object_reference": &mutation.checkpoint.object_reference,
        })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_event)
        .transpose()?;
    let Some(event) = event else {
        return Ok(false);
    };
    reconcile_receipt(
        store,
        &PublicationReceipt {
            sandbox: mutation.sandbox.clone(),
            checkpoint: mutation.checkpoint.clone(),
            event,
        },
        &mut session,
    )
    .await
}

async fn reconcile_receipt(
    store: &MongoSandboxStore,
    receipt: &PublicationReceipt,
    session: &mut ClientSession,
) -> Result<bool, SandboxStoreError> {
    let stored_sandbox = store
        .sandboxes
        .find_one(doc! { "_id": receipt.sandbox.id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_sandbox)
        .transpose()?;
    let stored_checkpoint = store
        .checkpoints
        .find_one(doc! {
            "_id": checkpoint_id(&receipt.sandbox.id, receipt.sandbox.revision)
        })
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .map(read_checkpoint)
        .transpose()?;
    Ok(
        crate::store::work_lease_update::sandbox_matches(stored_sandbox.as_ref(), &receipt.sandbox)
            && stored_checkpoint
                .as_ref()
                .is_some_and(|stored| stored.record == receipt.checkpoint)
            && reconcile_event_with_snapshot_session(store, &receipt.event, session)
                .await?
                .is_some(),
    )
}
