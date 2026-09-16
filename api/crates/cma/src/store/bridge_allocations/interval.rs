use layer2_core::bridge::allocation::{
    AllocationError, AllocationRecord, ALLOCATION_INTERVAL_MS, MAX_NEW_ALLOCATIONS_PER_INTERVAL,
};
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};

use super::{records::record_document, MongoBridgeAllocationStore, MutationError};

impl MongoBridgeAllocationStore {
    /// Called only for a new row, after exact receipt recovery and the shared
    /// Bridge write fence. Failed transactions consume no interval capacity.
    pub(super) async fn interval_document(
        &self,
        record: &AllocationRecord,
        session: &mut ClientSession,
    ) -> Result<Document, MutationError> {
        let admitted_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| AllocationError::Unavailable)?
            .as_millis();
        let admitted_at = i64::try_from(admitted_at).map_err(|_| AllocationError::Unavailable)?;
        let cutoff = admitted_at.saturating_sub(ALLOCATION_INTERVAL_MS);
        // No upper time bound: a backwards clock cannot release earlier slots.
        // Old rows predate the admission stamp and retain their creation time.
        // State, configuration generation and placement never reset the budget.
        let recent = self
            .records
            .count_documents(doc! {
                "bridge_id": record.identity.bridge_id.as_str(),
                "$or": [
                    {"interval_admitted_at_ms": {"$gt": cutoff}},
                    {"interval_admitted_at_ms": {"$exists": false},
                     "record.created_at_ms": {"$gt": cutoff}},
                ],
            })
            .limit(MAX_NEW_ALLOCATIONS_PER_INTERVAL)
            .session(&mut *session)
            .await?;
        if recent >= MAX_NEW_ALLOCATIONS_PER_INTERVAL {
            return Err(AllocationError::Capacity.into());
        }
        let mut document =
            crate::store::channel_history::new_document(record, record_document(record)?)?;
        // Store time, rather than a potentially delayed proposal's timestamp,
        // makes concurrent admissions share the same actual rolling interval.
        document.insert("interval_admitted_at_ms", admitted_at);
        Ok(document)
    }
}
