//! Bounded, complete-snapshot validation for Mongo agent logs.

use super::agent_log_telemetry::ScanCaller;
use layer2_core::ids::AgentId;
use layer2_core::sandbox::store::SandboxStoreError;
#[cfg(test)]
use layer2_core::sandbox::store::MAX_AGENT_LOG_BATCH_SIZE;
use layer2_core::sandbox::{Agent, AgentLogEntry};
use mongodb::bson::{doc, Binary, Bson, Document, RawDocumentBuf};
use sha2::{Digest, Sha256};

use super::agent_log::{log_document, log_id, read_entry};
use super::sandbox_mongo::{signed, unsigned, MongoSandboxStore};

pub(super) use cma_am_agent_log::capacity::{ensure_append_batch_size, AppendCapacity};
#[cfg(test)]
use cma_am_agent_log::capacity::{
    DELETE_RESERVE_BYTES, DELETE_RESERVE_ENTRIES, STOP_AGENT_LOG_BYTES, STOP_AGENT_LOG_ENTRIES,
    STOP_RESERVE_BYTES, STOP_RESERVE_ENTRIES,
};
use cma_am_agent_log::capacity::{
    MAX_AGENT_LOG_DOCUMENT_BYTES, PHYSICAL_AGENT_LOG_BYTES, PHYSICAL_AGENT_LOG_ENTRIES,
};
#[cfg(test)]
pub(super) use cma_am_agent_log::capacity::{NORMAL_AGENT_LOG_BYTES, NORMAL_AGENT_LOG_ENTRIES};
pub(super) const INTEGRITY_FIELD: &str = "agent_log_integrity_v1";
pub(super) const QUARANTINE_FIELD: &str = "agent_log_integrity_quarantine_v1";
const SEMANTIC_FIELD: &str = "semantic_checkpoint_v1";
const INTEGRITY_FORMAT: i64 = 1;
const HASH_BYTES: usize = 32;

#[derive(Debug)]
pub(super) enum TransactionLogError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

#[derive(Default)]
#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
pub(super) struct LogSummary {
    entry_count: u64,
    encoded_bytes: usize,
    head_hash: [u8; HASH_BYTES],
    semantic: Option<SemanticCheckpoint>,
}

impl LogSummary {
    pub(super) const fn entry_count(&self) -> u64 {
        self.entry_count
    }

    pub(super) fn semantic(&self) -> Option<&SemanticCheckpoint> {
        self.semantic.as_ref()
    }

    pub(super) fn with_semantic(mut self, semantic: SemanticCheckpoint) -> Self {
        self.semantic = Some(semantic);
        self
    }
}

pub(crate) mod fleet;
mod metadata;
mod preparation;
mod proof;
mod recovery;
pub(super) use recovery::exact_recovery_head;
#[cfg(test)]
mod quarantine_tests;
pub(super) mod read;
mod scan;
mod semantic;
#[cfg(test)]
mod semantic_tests;
#[cfg(test)]
mod tests;

pub(super) use layer2_core::sandbox::AgentLogCheckpoint as SemanticCheckpoint;
pub(super) use metadata::parse_integrity;
use metadata::{hash_raw, history_identity, LogIntegrity};
pub(super) use proof::{trusted_summary, trusted_summary_at_head};
pub(super) use scan::{
    scan_collection, scan_complete_log_with_policy, IntegrityFailurePolicy, ScanOptions,
};
pub(super) use scan::{
    scan_complete_log_with_session, scan_complete_log_with_snapshot_policy,
    scan_complete_log_with_snapshot_session,
};
pub(super) use semantic::audit_checkpoint;

pub(super) fn empty_summary() -> LogSummary {
    LogSummary::default()
}

pub(super) fn integrity_document(
    agent: &Agent,
    summary: &LogSummary,
    bound_agent_revision: u64,
    semantic: SemanticCheckpoint,
) -> Result<Document, SandboxStoreError> {
    LogIntegrity::from_summary(agent, summary, bound_agent_revision, semantic).to_document()
}

/// Carry a validated integrity summary across an Agent revision-only mutation.
/// The caller must still CAS the old revision/head; a changed log head is not
/// eligible for carry and will be repaired by the next append fallback.
pub(super) fn carry_integrity_for_revision(
    current_row: &Document,
    current: &Agent,
    next: &Agent,
    fields: &mut Document,
) -> Result<(), SandboxStoreError> {
    if current.conversation.last_seq != next.conversation.last_seq {
        return Ok(());
    }
    let Some(mut integrity) = parse_integrity(current_row)? else {
        return Ok(());
    };
    if integrity.bound_agent_revision != current.revision
        || integrity.history_identity != history_identity(current)
        || history_identity(current) != history_identity(next)
        || current.conversation != next.conversation
        || integrity.head_seq != current.conversation.last_seq
        || integrity.semantic.through_seq != current.conversation.last_seq
    {
        return Ok(());
    }
    integrity.bound_agent_revision = next.revision;
    fields.insert(INTEGRITY_FIELD, integrity.to_document()?);
    Ok(())
}

pub(super) struct PreparedAppend {
    pub(super) integrity: Document,
    pub(super) documents: Vec<RawDocumentBuf>,
}

/// Allocate/validate sequences before encoding. Capacity, hash and Mongo insert
/// all use these same immutable BSON buffers, including the padded `_id`.
pub(super) fn prepare_append(
    agent: &Agent,
    summary: &LogSummary,
    appended: &[AgentLogEntry],
    bound_agent_revision: u64,
    semantic: SemanticCheckpoint,
    capacity: AppendCapacity,
) -> Result<PreparedAppend, SandboxStoreError> {
    ensure_append_batch_size(appended.len())?;
    let mut documents = Vec::with_capacity(appended.len());
    for (offset, entry) in appended.iter().enumerate() {
        let expected = summary
            .entry_count
            .checked_add(offset as u64)
            .and_then(|value| value.checked_add(1))
            .ok_or(SandboxStoreError::SequenceConflict)?;
        if entry.agent_id != agent.id || entry.seq != expected {
            return Err(SandboxStoreError::SequenceConflict);
        }
        let raw = RawDocumentBuf::try_from(&log_document(entry)?)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        documents.push(raw);
    }
    ensure_append_capacity(summary, &documents, capacity)?;
    let total_bytes = documents
        .iter()
        .try_fold(summary.encoded_bytes, |total, raw| {
            total
                .checked_add(raw.as_bytes().len())
                .ok_or(SandboxStoreError::AgentLogCapacityExceeded)
        })?;
    let head_seq = summary
        .entry_count
        .checked_add(documents.len() as u64)
        .ok_or(SandboxStoreError::SequenceConflict)?;
    if semantic.through_seq != head_seq {
        return Err(SandboxStoreError::Corrupt(
            "append checkpoint does not cover the final log head".into(),
        ));
    }
    let integrity = LogIntegrity {
        head_seq,
        entry_count: head_seq,
        encoded_bytes: total_bytes,
        head_hash: documents
            .last()
            .map_or(summary.head_hash, |raw| hash_raw(raw.as_bytes())),
        bound_agent_revision,
        history_identity: history_identity(agent),
        semantic,
    }
    .to_document()?;
    Ok(PreparedAppend {
        integrity,
        documents,
    })
}

pub(super) fn ensure_append_capacity(
    complete: &LogSummary,
    appended: &[RawDocumentBuf],
    capacity: AppendCapacity,
) -> Result<(), SandboxStoreError> {
    if appended.is_empty() {
        return Ok(());
    }
    ensure_append_batch_size(appended.len())?;
    let pending_count = u64::try_from(appended.len()).map_err(|_| capacity_error())?;
    let mut pending_bytes = 0_usize;
    for document in appended {
        let document_bytes = document.as_bytes().len();
        if document_bytes > MAX_AGENT_LOG_DOCUMENT_BYTES {
            tracing::warn!(
                limit_kind = "document_bytes",
                document_bytes,
                max_document_bytes = MAX_AGENT_LOG_DOCUMENT_BYTES,
                "Agent log capacity rejected"
            );
            return Err(capacity_error());
        }
        pending_bytes = pending_bytes
            .checked_add(document_bytes)
            .ok_or_else(capacity_error)?;
    }
    if capacity_allows(
        complete.entry_count,
        complete.encoded_bytes,
        pending_count,
        pending_bytes,
        capacity,
    ) {
        Ok(())
    } else {
        let (max_entries, max_bytes) = capacity.limits();
        tracing::warn!(
            ?capacity,
            limit_kind = "retained_log",
            existing_entries = complete.entry_count,
            existing_bytes = complete.encoded_bytes,
            pending_entries = pending_count,
            pending_bytes,
            max_entries,
            max_bytes,
            "Agent log capacity rejected"
        );
        Err(capacity_error())
    }
}

/// Refuse recovery near normal capacity before any native reopen. Startup
/// metadata and the reopen/publication suffix need ordinary log capacity; the
/// stop/delete reserves are never borrowed to make a failed Agent usable.
pub(super) fn ensure_reopen_capacity(summary: &LogSummary) -> Result<(), SandboxStoreError> {
    ensure_reopen_capacity_for(summary.entry_count, summary.encoded_bytes)
}

pub(in crate::store) fn ensure_reopen_capacity_for(
    entry_count: u64,
    encoded_bytes: usize,
) -> Result<(), SandboxStoreError> {
    capacity_allows(
        entry_count,
        encoded_bytes,
        64,
        512 * 1024,
        AppendCapacity::Normal,
    )
    .then_some(())
    .ok_or_else(capacity_error)
}

fn capacity_allows(
    existing_count: u64,
    existing_bytes: usize,
    pending_count: u64,
    pending_bytes: usize,
    capacity: AppendCapacity,
) -> bool {
    let (max_count, max_bytes) = capacity.limits();
    existing_count
        .checked_add(pending_count)
        .is_some_and(|count| count <= max_count)
        && existing_bytes
            .checked_add(pending_bytes)
            .is_some_and(|bytes| bytes <= max_bytes)
}

fn capacity_error() -> SandboxStoreError {
    SandboxStoreError::AgentLogCapacityExceeded
}

impl From<SandboxStoreError> for TransactionLogError {
    fn from(error: SandboxStoreError) -> Self {
        Self::Domain(error)
    }
}

#[cfg(test)]
#[path = "agent_log_integrity/reopen_capacity_tests.rs"]
mod reopen_capacity_tests;
