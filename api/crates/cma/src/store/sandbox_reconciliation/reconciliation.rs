use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::bson::{doc, Timestamp};
use mongodb::options::ReadConcern;

use super::super::agent_log::read_entry;
use super::super::agents::read_agent;
use super::super::sandbox_events::reconcile_event_with_snapshot_session;
use super::super::sandbox_mongo::{signed, MongoSandboxStore};
use super::super::sandboxes::read_sandbox;
use super::transaction::RecoveryReceipt;

pub(super) async fn reconcile(
    store: &MongoSandboxStore,
    receipt: &RecoveryReceipt,
    snapshot_time: Timestamp,
) -> Result<bool, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(snapshot_time)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let sandbox = store
        .sandboxes
        .find_one(doc! { "_id": receipt.sandbox.id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_sandbox)
        .transpose()?;
    if sandbox.as_ref() != Some(&receipt.sandbox) {
        return Ok(false);
    }
    for recorded in &receipt.segmented {
        let original_logs = receipt
            .logs
            .iter()
            .filter(|entry| entry.agent_id == recorded.next.id);
        if !receipt.agents.contains(&recorded.next)
            || !original_logs.eq(recorded.logs.iter())
            || !recorded.matches(store, &mut session).await?
        {
            return Ok(false);
        }
    }
    for expected in &receipt.agents {
        let actual = store
            .agents
            .find_one(doc! { "_id": expected.id.as_str() })
            .read_concern(ReadConcern::snapshot())
            .session(&mut session)
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
        let actual = store.agent_log.find_one(doc! {
            "agent_id": expected.agent_id.as_str(),
            "seq": signed(expected.seq, "reconciled runtime-loss log sequence").map_err(SandboxStoreError::Corrupt)?,
        }).read_concern(ReadConcern::snapshot()).session(&mut session).await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
            .as_ref().map(read_entry).transpose()?;
        if actual.as_ref() != Some(expected) {
            return Ok(false);
        }
    }
    for expected in &receipt.events {
        if reconcile_event_with_snapshot_session(store, expected, &mut session)
            .await?
            .is_none()
        {
            return Ok(false);
        }
    }
    Ok(true)
}
