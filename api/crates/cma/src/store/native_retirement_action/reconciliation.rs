use super::*;
use crate::store::sandbox_mongo::wait_for_reconciliation;
use mongodb::bson::Timestamp;

pub(super) async fn prove_commit(
    store: &MongoSandboxStore,
    parent: &SandboxId,
    expected: Option<&RetirementAction>,
    operation_time: Option<Timestamp>,
    error: mongodb::error::Error,
) -> Result<(), SandboxStoreError> {
    let snapshot_time = operation_time.ok_or_else(|| {
        SandboxStoreError::Unavailable(
            "retirement custody commit remained unknown without an operation time".into(),
        )
    })?;
    // A fresh majority read may still be behind the unacknowledged write.
    // Read a snapshot at the transaction's operation time before dispatch.
    let store = store.clone();
    let parent = parent.clone();
    let expected = expected.cloned();
    wait_for_reconciliation("retirement custody commit", async move {
        let mut session = store
            .client
            .start_session()
            .snapshot(true)
            .snapshot_time(snapshot_time)
            .await
            .map_err(unavailable)?;
        let row = store
            .sandboxes
            .find_one(doc! {"_id": parent.as_str()})
            .projection(doc! {FIELD: 1})
            .read_concern(ReadConcern::snapshot())
            .session(&mut session)
            .await
            .map_err(unavailable)?
            .ok_or(SandboxStoreError::Conflict)?;
        if action_from(&row)? == expected {
            Ok(())
        } else {
            Err(unavailable(error))
        }
    })
    .await
    .map_err(|deadline| SandboxStoreError::Unavailable(deadline.to_string()))?
}
