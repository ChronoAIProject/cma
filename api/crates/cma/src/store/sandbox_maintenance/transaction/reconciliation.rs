use mongodb::options::ReadConcern;

use super::*;
use crate::store::sandbox_events::reconcile_event_with_snapshot_session;

pub(in crate::store::sandbox_maintenance) async fn reconcile(
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
