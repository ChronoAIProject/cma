//! Independent erasure evidence for missing admitted Trigger inputs.
use super::{documents, driver_error, MongoTriggerDefinitionStore};
use cma_am_triggers::definitions::revision_fenced;
use layer2_core::trigger_domain::{PublicTriggerId, TriggerId, TriggerStoreError};
use mongodb::bson::doc;

pub(super) async fn prove(
    store: &MongoTriggerDefinitionStore,
    id: &TriggerId,
    digest: &str,
    public_id: &PublicTriggerId,
) -> Result<bool, TriggerStoreError> {
    let row = store
        .revisions
        .find_one(doc! {"trigger_id":id.as_str(),"record.digest":digest})
        .sort(doc! {"revision":1_i32})
        .await
        .map_err(|error| driver_error("read publisher erasure evidence", error))?;
    if let Some(row) = row {
        // Validate the complete immutable record before trusting any marker.
        let stored = documents::read_revision(&row)?;
        if stored.trigger_id != id.as_str()
            || stored.revision.digest != digest
            || &stored.public_id != public_id
        {
            return Err(TriggerStoreError::Corrupt(
                "publisher erasure evidence crossed its Launch identity".into(),
            ));
        }
        if stored.revision.publisher_deleted || revision_fenced(&row)? {
            return Ok(true);
        }
    }
    let database = store.client.database(&store.definitions.namespace().db);
    crate::store::account_deletions::trigger_erasure::prove(&database, id)
        .await
        .map_err(|error| match error {
            crate::store::account_deletions::trigger_erasure::Error::Driver(error) => {
                driver_error("read durable publisher erasure record", error)
            }
            crate::store::account_deletions::trigger_erasure::Error::Corrupt => {
                TriggerStoreError::Corrupt("invalid durable publisher erasure record".into())
            }
        })
}
