//! An uncertain backfill may recover its immutable receipt, never reinsert it.
use super::*;
use crate::store::{agent_provenance::read, sandbox_mongo::wait_for_reconciliation};
use mongodb::{bson::Timestamp, options::ReadConcern};

pub(super) async fn after_unknown(
    store: &MongoSandboxStore,
    expected: AgentCreationProvenance,
    at: Option<Timestamp>,
) -> Result<ProvenanceBackfillOutcome, SandboxStoreError> {
    let at = at.ok_or_else(unavailable)?;
    let store = store.clone();
    wait_for_reconciliation("Agent provenance backfill commit", async move {
        let mut session = store
            .client
            .start_session()
            .snapshot(true)
            .snapshot_time(at)
            .await
            .map_err(|_| unavailable())?;
        let row = store
            .agent_creation_provenance
            .find_one(doc! {"_id":expected.agent_id.as_str()})
            .read_concern(ReadConcern::snapshot())
            .session(&mut session)
            .await
            .map_err(|_| unavailable())?
            .ok_or_else(unavailable)?;
        if read(&row)? != expected {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(ProvenanceBackfillOutcome::Retained)
    })
    .await
    .map_err(SandboxStoreError::Unavailable)?
}

#[cfg(test)]
#[path = "proof_tests.rs"]
mod proof_tests;
