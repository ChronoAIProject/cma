//! Retain already acknowledged private text when a compact failure cannot flush it.
use super::*;
use layer2_core::sandbox::store::AgentConversationCapacitySettlement;
use mongodb::{bson::Document, ClientSession};

pub(super) const FIELD: &str = "agent_capacity_prefix_v1";

pub(in crate::store) async fn preserve(
    store: &MongoSandboxStore,
    row: &Document,
    proof: &AgentConversationCapacitySettlement,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let Some(prefix) = proof.retained_prefix() else {
        return Ok(());
    };
    prefix.validate().map_err(ApplyError::Domain)?;
    let value = super::super::sandbox_mongo::record_bson(prefix)
        .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error)))?;
    if row.get(FIELD).is_some_and(|actual| actual != &value) {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    // This participates in the exact Agent/checkpoint CAS transaction, and is
    // never cleared by active checkpoint carry, replacement or cleanup.
    let saved = store
        .agents
        .update_one(
            doc! {"_id": proof.current().id.as_str()},
            doc! {"$set": {FIELD: value}},
        )
        .session(session)
        .await
        .map_err(ApplyError::Driver)?;
    if saved.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    Ok(())
}

pub(super) fn matches(
    row: &Document,
    proof: &AgentConversationCapacitySettlement,
) -> Result<bool, SandboxStoreError> {
    let Some(prefix) = proof.retained_prefix() else {
        return Ok(true);
    };
    let expected =
        super::super::sandbox_mongo::record_bson(prefix).map_err(SandboxStoreError::Corrupt)?;
    Ok(row.get(FIELD) == Some(&expected))
}
