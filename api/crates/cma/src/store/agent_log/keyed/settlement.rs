//! Settle the original attempted transaction without reconstructing its result.
use super::*;
use mongodb::bson::Timestamp;

pub(in crate::store) async fn after_unknown(
    store: &MongoSandboxStore,
    operation: &PublicationIdentity,
    entry: &AgentLogEntry,
    attempted: &Attempt,
    snapshot_time: Option<Timestamp>,
) -> Result<AgentLogEntry, SandboxStoreError> {
    let at = snapshot_time.ok_or_else(unresolved)?;
    let mut snapshot = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(at)
        .await
        .map_err(driver)?;
    // Persist and suggestion-domain writes used the same transaction. The exact
    // immutable publication is evidence of that transaction, not just of a log
    // entry with the same kind observed against a different current head.
    match writer_reconcile::reconcile(
        store,
        operation,
        &attempted.next,
        std::slice::from_ref(entry),
        &publication::result_digest(entry)?,
        &mut snapshot,
    )
    .await?
    {
        writer_reconcile::Outcome::Recorded(actual) if *actual == attempted.publication => {
            Ok(entry.clone())
        }
        writer_reconcile::Outcome::Recorded(_) => Err(SandboxStoreError::Conflict),
        writer_reconcile::Outcome::Unknown => Err(unresolved()),
    }
}

fn unresolved() -> SandboxStoreError {
    SandboxStoreError::Unavailable(
        "keyed Agent log publication remains unknown; retain its original identity".into(),
    )
}
