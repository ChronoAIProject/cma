//! BSON envelope helpers shared by the Agent-log readers and writers.

#[cfg(test)]
use layer2_core::ids::AgentId;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::AgentLogEntry;
use mongodb::bson::Document;

use super::sandbox_mongo::DOCUMENT_SCHEMA_VERSION;
pub(super) use cma_am_agent_log::legacy::log_id;

pub(super) fn log_document(entry: &AgentLogEntry) -> Result<Document, SandboxStoreError> {
    cma_am_agent_log::legacy::log_document(entry, DOCUMENT_SCHEMA_VERSION)
}

pub(super) fn read_entry(document: &Document) -> Result<AgentLogEntry, SandboxStoreError> {
    cma_am_agent_log::legacy::read_entry(document, DOCUMENT_SCHEMA_VERSION)
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
