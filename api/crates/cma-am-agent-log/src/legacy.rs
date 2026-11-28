//! BSON envelope helpers shared by the Agent-log readers and writers.

use layer2_core::ids::AgentId;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::AgentLogEntry;
use mongodb::bson::{doc, Document};

use cma_am_mongo_primitives::{read_record, record_bson, signed};

pub fn log_id(id: &AgentId, seq: u64) -> String {
    format!("{}:{seq:020}", id.as_str())
}

pub fn log_document(
    entry: &AgentLogEntry,
    schema_version: i64,
) -> Result<Document, SandboxStoreError> {
    Ok(doc! {
        "_id": log_id(&entry.agent_id, entry.seq),
        "document_schema_version": schema_version,
        "agent_id": entry.agent_id.as_str(),
        "seq": signed(entry.seq, "agent log sequence").map_err(SandboxStoreError::Corrupt)?,
        "record": record_bson(entry).map_err(SandboxStoreError::Corrupt)?,
    })
}

pub fn read_entry(
    document: &Document,
    schema_version: i64,
) -> Result<AgentLogEntry, SandboxStoreError> {
    let entry: AgentLogEntry =
        read_record(document, schema_version).map_err(SandboxStoreError::Corrupt)?;
    let seq = signed(entry.seq, "agent log sequence").map_err(SandboxStoreError::Corrupt)?;
    if document.get_str("_id").ok() != Some(log_id(&entry.agent_id, entry.seq).as_str())
        || document.get_str("agent_id").ok() != Some(entry.agent_id.as_str())
        || document.get_i64("seq").ok() != Some(seq)
    {
        return Err(SandboxStoreError::Corrupt(
            "agent log query fields disagree with its record".into(),
        ));
    }
    Ok(entry)
}
