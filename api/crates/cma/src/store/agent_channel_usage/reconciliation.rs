//! The immutable event proves the transaction; absence never permits reapply.
use super::*;
use crate::store::sandbox_mongo::wait_for_reconciliation;
use mongodb::{bson::Timestamp, options::ReadConcern};

pub(super) async fn verify(
    store: &MongoSandboxStore,
    request: &ChannelUsageMutation,
    expected: ChannelUsageEvent,
    at: Option<Timestamp>,
) -> Result<ChannelUsageEvent, SandboxStoreError> {
    let at = at.ok_or_else(unavailable)?;
    let store = store.clone();
    let request = request.clone();
    wait_for_reconciliation("Agent channel usage commit", async move {
        let mut session = store
            .client
            .start_session()
            .snapshot(true)
            .snapshot_time(at)
            .await
            .map_err(|_| unavailable())?;
        let row = store
            .channel_usage_events
            .find_one(doc! {"_id":request.operation_id()})
            .read_concern(ReadConcern::snapshot())
            .session(&mut session)
            .await
            .map_err(|_| unavailable())?
            .ok_or_else(unavailable)?;
        let actual = request.replay(read_event(&row)?)?;
        if actual != expected {
            return Err(SandboxStoreError::Conflict);
        }
        // A later event can advance the summary while acknowledgements are lost.
        // Validate retained history without replacing the captured request fence.
        let retained =
            records::read_retained_state(&store, &request.creation.agent_id, &mut session, true)
                .await
                .map_err(map_error)?
                .ok_or_else(unavailable)?;
        if retained.state.creation != expected.after.creation
            || retained.state.revision < expected.after.revision
        {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(expected)
    })
    .await
    .map_err(SandboxStoreError::Unavailable)?
}

#[cfg(test)]
#[path = "proof_tests.rs"]
mod proof_tests;
