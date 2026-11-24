use super::super::sandbox_events::reconcile_event_with_snapshot_session;
use super::super::sandbox_mongo::MongoSandboxStore;
use super::super::sandboxes::read_sandbox;
use super::transaction::MaintenanceReceipt;
use layer2_core::sandbox::SandboxStoreError;
use mongodb::bson::doc;
use mongodb::options::ReadConcern;

pub(super) async fn reconcile(
    store: &MongoSandboxStore,
    receipt: &MaintenanceReceipt,
    snapshot_time: mongodb::bson::Timestamp,
) -> Result<bool, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(snapshot_time)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let actual = store
        .sandboxes
        .find_one(doc! { "_id": receipt.sandbox.id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_sandbox)
        .transpose()?;
    if actual.as_ref() != Some(&receipt.sandbox) {
        return Ok(false);
    }
    match &receipt.event {
        Some(event) => Ok(
            reconcile_event_with_snapshot_session(store, event, &mut session)
                .await?
                .is_some(),
        ),
        None => Ok(false),
    }
}
