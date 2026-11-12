//! BSON envelope helpers shared by the Agent-log readers and writers.

use layer2_core::ids::AgentId;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::AgentLogEntry;
use mongodb::bson::{doc, Document};

use super::sandbox_mongo::{
    i64_field_matches, read_record, record_bson, signed, string_field_matches,
    DOCUMENT_SCHEMA_VERSION,
};

pub(super) fn log_id(id: &AgentId, seq: u64) -> String {
    format!("{}:{seq:020}", id.as_str())
}

pub(super) fn log_document(entry: &AgentLogEntry) -> Result<Document, SandboxStoreError> {
    Ok(doc! {
        "_id": log_id(&entry.agent_id, entry.seq),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "agent_id": entry.agent_id.as_str(),
        "seq": signed(entry.seq, "agent log sequence").map_err(SandboxStoreError::Corrupt)?,
        "record": record_bson(entry).map_err(SandboxStoreError::Corrupt)?,
    })
}

pub(super) fn read_entry(document: &Document) -> Result<AgentLogEntry, SandboxStoreError> {
    let entry: AgentLogEntry = read_record(document).map_err(SandboxStoreError::Corrupt)?;
    let seq = signed(entry.seq, "agent log sequence").map_err(SandboxStoreError::Corrupt)?;
    if !string_field_matches(document, "_id", &log_id(&entry.agent_id, entry.seq))
        || !string_field_matches(document, "agent_id", entry.agent_id.as_str())
        || !i64_field_matches(document, "seq", seq)
    {
        return Err(SandboxStoreError::Corrupt(
            "agent log query fields disagree with its record".into(),
        ));
    }
    Ok(entry)
}

#[cfg(test)]
pub(super) fn verify_complete_log(
    agent_id: &AgentId,
    last_seq: u64,
    logs: &[AgentLogEntry],
) -> Result<(), SandboxStoreError> {
    let expected_len =
        usize::try_from(last_seq).map_err(|_| SandboxStoreError::SequenceConflict)?;
    if logs.len() != expected_len
        || logs
            .iter()
            .enumerate()
            .any(|(offset, entry)| entry.agent_id != *agent_id || entry.seq != offset as u64 + 1)
    {
        return Err(SandboxStoreError::SequenceConflict);
    }
    Ok(())
}
