//! Require a server-confirmed snapshot without manufacturing one on an ordinary session.
use super::*;

pub(super) async fn agent(
    store: &MongoSandboxStore,
    current: &Agent,
    session: &mut ClientSession,
) -> Result<(Document, Timestamp), TransactionLogError> {
    // runCommand does not inherit the collection's majority concern. Leave the
    // concern absent so only an actual snapshot session adds snapshot isolation.
    // Forcing snapshot concern here also populates snapshot_time on ordinary
    // sessions, whose later reads are not automatically pinned by the driver.
    let namespace = store.agents.namespace();
    let response = store
        .client
        .database(&namespace.db)
        .run_command(doc! {
            "find": &namespace.coll,
            "filter": {"_id": current.id.as_str()},
            "limit": 1_i64,
            "singleBatch": true,
        })
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    let cursor = response.get_document("cursor").map_err(|_| pending())?;
    let at = cursor
        .get_timestamp("atClusterTime")
        .map_err(|_| pending())?;
    if session.snapshot_time() != Some(at) || cursor.get_i64("id").map_err(|_| pending())? != 0 {
        return Err(pending());
    }
    let batch = cursor.get_array("firstBatch").map_err(|_| pending())?;
    if batch.len() != 1 {
        return Err(super::super::corrupt().into());
    }
    let row = batch[0].as_document().ok_or_else(pending)?.clone();
    Ok((row, at))
}
