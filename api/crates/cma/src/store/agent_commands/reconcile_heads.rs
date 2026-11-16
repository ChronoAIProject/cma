//! Narrow, nontransactional hints. Malformed rows are deliberately absent so
//! only the complete reconciliation path can diagnose or change their state.
use futures_util::TryStreamExt;
use layer2_core::sandbox::agent_io::{
    CommandLedgerError, CommandReconcileHead, MAX_COMMAND_RECONCILE_HEADS,
};
use layer2_core::sandbox::AgentStatus;
use layer2_core::AgentId;
use mongodb::bson::{doc, Bson, Document};
use std::time::Duration;

use crate::store::sandbox_mongo::{require_version, MongoSandboxStore};

pub(super) async fn read(
    store: &MongoSandboxStore,
    ids: &[AgentId],
) -> Result<Vec<CommandReconcileHead>, CommandLedgerError> {
    if ids.len() > MAX_COMMAND_RECONCILE_HEADS {
        return Err(CommandLedgerError::Invalid);
    }
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut cursor = store
        .agents
        .find(doc! { "_id": { "$in": ids.iter().map(AgentId::as_str).collect::<Vec<_>>() } })
        .projection(doc! { "status": 1, "document_schema_version": 1,
        "record.conversation.last_seq": 1, "record.conversation.runtime_generation": 1,
        "record.conversation.active_turn": 1 })
        .limit(MAX_COMMAND_RECONCILE_HEADS as i64)
        .max_time(Duration::from_millis(500))
        .await
        .map_err(|_| CommandLedgerError::Unavailable)?;
    let mut heads = Vec::new();
    while let Some(row) = cursor
        .try_next()
        .await
        .map_err(|_| CommandLedgerError::Unavailable)?
    {
        if let Some(head) = decode(&row) {
            heads.push(head);
        }
    }
    Ok(heads)
}

fn unsigned(value: &Bson) -> Option<u64> {
    match value {
        Bson::Int32(value) => u64::try_from(*value).ok(),
        Bson::Int64(value) => u64::try_from(*value).ok(),
        _ => None,
    }
}

fn optional_unsigned(value: &Bson) -> Option<Option<u64>> {
    match value {
        Bson::Null => Some(None),
        value => unsigned(value).map(Some),
    }
}

fn decode(row: &Document) -> Option<CommandReconcileHead> {
    require_version(row).ok()?;
    let agent_id = row.get_str("_id").ok()?;
    if agent_id.is_empty() {
        return None;
    }
    let status: AgentStatus =
        mongodb::bson::deserialize_from_bson(row.get("status")?.clone()).ok()?;
    let conversation = row
        .get_document("record")
        .ok()?
        .get_document("conversation")
        .ok()?;
    Some(CommandReconcileHead {
        agent_id: AgentId(agent_id.into()),
        last_seq: unsigned(conversation.get("last_seq")?)?,
        live: status.is_live(),
        runtime_generation: optional_unsigned(conversation.get("runtime_generation")?)?,
        active_turn: optional_unsigned(conversation.get("active_turn")?)?,
    })
}

#[cfg(test)]
#[path = "reconcile_heads_tests.rs"]
mod tests;
