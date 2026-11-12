//! Exact original lifecycle effects in the original commit snapshot.
use super::*;

pub(super) async fn after_unknown(
    store: &MongoSandboxStore,
    receipt: &LifecycleReceipt,
    snapshot_time: Timestamp,
) -> Result<bool, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(snapshot_time)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    snapshot(store, receipt, &mut session).await
}

pub(in crate::store) async fn snapshot(
    store: &MongoSandboxStore,
    receipt: &LifecycleReceipt,
    session: &mut mongodb::ClientSession,
) -> Result<bool, SandboxStoreError> {
    let sandbox = store
        .sandboxes
        .find_one(doc! { "_id": receipt.sandbox.id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_sandbox)
        .transpose()?;
    if sandbox.as_ref() != Some(&receipt.sandbox) {
        return Ok(false);
    }
    if !checkpoint_receipt::matches(store, &receipt.checkpoint_children, session).await? {
        return Ok(false);
    }
    for recorded in &receipt.segmented {
        if !receipt.agents.contains(&recorded.next) || !recorded.matches(store, session).await? {
            return Ok(false);
        }
    }
    for expected in &receipt.agents {
        let actual = store
            .agents
            .find_one(doc! { "_id": expected.id.as_str() })
            .read_concern(ReadConcern::snapshot())
            .session(&mut *session)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
            .as_ref()
            .map(read_agent)
            .transpose()?;
        if actual.as_ref() != Some(expected) {
            return Ok(false);
        }
    }
    for expected in &receipt.logs {
        let actual = store
            .agent_log
            .find_one(doc! {
                "agent_id": expected.agent_id.as_str(),
                "seq": signed(expected.seq, "reconciled lifecycle log sequence")
                    .map_err(SandboxStoreError::Corrupt)?,
            })
            .read_concern(ReadConcern::snapshot())
            .session(&mut *session)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
            .as_ref()
            .map(read_entry)
            .transpose()?;
        if actual.as_ref() != Some(expected) {
            return Ok(false);
        }
    }
    for expected in &receipt.events {
        if reconcile_event_with_snapshot_session(store, expected, session)
            .await?
            .is_none()
        {
            return Ok(false);
        }
    }
    Ok(true)
}
