//! Bounded, complete-snapshot validation for Mongo agent logs.

use futures_util::TryStreamExt;
use layer2_core::ids::AgentId;
use layer2_core::sandbox::store::{SandboxStoreError, MAX_AGENT_LOG_BATCH_SIZE};
use layer2_core::sandbox::AgentLogEntry;
use mongodb::bson::{Document, RawDocumentBuf};
use mongodb::options::ReadConcern;

use super::agent_log::{log_document, read_entry};
use super::sandbox_mongo::MongoSandboxStore;

pub(super) const NORMAL_AGENT_LOG_ENTRIES: u64 = 10_000;
pub(super) const NORMAL_AGENT_LOG_BYTES: usize = 64 * 1024 * 1024;
const STOP_RESERVE_ENTRIES: u64 = 64;
const STOP_RESERVE_BYTES: usize = 512 * 1024;
const DELETE_RESERVE_ENTRIES: u64 = 64;
const DELETE_RESERVE_BYTES: usize = 512 * 1024;
const STOP_AGENT_LOG_ENTRIES: u64 = NORMAL_AGENT_LOG_ENTRIES + STOP_RESERVE_ENTRIES;
const STOP_AGENT_LOG_BYTES: usize = NORMAL_AGENT_LOG_BYTES + STOP_RESERVE_BYTES;
const PHYSICAL_AGENT_LOG_ENTRIES: u64 = STOP_AGENT_LOG_ENTRIES + DELETE_RESERVE_ENTRIES;
const PHYSICAL_AGENT_LOG_BYTES: usize = STOP_AGENT_LOG_BYTES + DELETE_RESERVE_BYTES;
const MAX_AGENT_LOG_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;

pub(super) enum TransactionLogError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AppendCapacity {
    Normal,
    StopOrFailure,
    Delete,
}

impl AppendCapacity {
    fn limits(self) -> (u64, usize) {
        match self {
            Self::Normal => (NORMAL_AGENT_LOG_ENTRIES, NORMAL_AGENT_LOG_BYTES),
            Self::StopOrFailure => (STOP_AGENT_LOG_ENTRIES, STOP_AGENT_LOG_BYTES),
            Self::Delete => (PHYSICAL_AGENT_LOG_ENTRIES, PHYSICAL_AGENT_LOG_BYTES),
        }
    }
}

pub(super) struct LogSummary {
    entry_count: u64,
    encoded_bytes: usize,
}

impl LogSummary {
    pub(super) const fn entry_count(&self) -> u64 {
        self.entry_count
    }
}

pub(super) async fn scan_complete_log<F>(
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    last_seq: u64,
    session: &mut mongodb::ClientSession,
    snapshot: bool,
    visit: F,
) -> Result<LogSummary, TransactionLogError>
where
    F: FnMut(AgentLogEntry) -> Result<(), SandboxStoreError>,
{
    scan_collection(
        &store.agent_log,
        agent_id,
        last_seq,
        session,
        snapshot,
        visit,
    )
    .await
}

pub(super) async fn scan_collection<F>(
    collection: &mongodb::Collection<Document>,
    agent_id: &AgentId,
    last_seq: u64,
    session: &mut mongodb::ClientSession,
    snapshot: bool,
    mut visit: F,
) -> Result<LogSummary, TransactionLogError>
where
    F: FnMut(AgentLogEntry) -> Result<(), SandboxStoreError>,
{
    let mut observation = super::agent_log_telemetry::Scan::new();
    if last_seq > PHYSICAL_AGENT_LOG_ENTRIES {
        tracing::warn!(
            limit_kind = "physical_entries",
            existing_entries = last_seq,
            max_entries = PHYSICAL_AGENT_LOG_ENTRIES,
            "Agent log capacity rejected"
        );
        return Err(capacity_error().into());
    }
    let raw_log = collection.clone_with_type::<RawDocumentBuf>();
    let find = raw_log
        .find(mongodb::bson::doc! { "agent_id": agent_id.as_str() })
        .sort(mongodb::bson::doc! { "seq": 1 })
        .limit(i64::try_from(PHYSICAL_AGENT_LOG_ENTRIES + 1).unwrap_or(i64::MAX));
    let find = if snapshot {
        find.read_concern(ReadConcern::snapshot())
    } else {
        find
    };
    let mut cursor = find
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    let mut entry_count = 0_u64;
    let mut encoded_bytes = 0_usize;
    while let Some(raw) = cursor
        .stream(session)
        .try_next()
        .await
        .map_err(TransactionLogError::Driver)?
    {
        let document_bytes = raw.as_bytes().len();
        observation.document(document_bytes);
        let document = Document::try_from(&raw).map_err(|error| {
            TransactionLogError::Domain(SandboxStoreError::Corrupt(error.to_string()))
        })?;
        let entry = read_entry(&document).map_err(TransactionLogError::Domain)?;
        let expected = entry_count.checked_add(1);
        if expected != Some(entry.seq) || entry.seq > last_seq {
            return Err(TransactionLogError::Domain(
                SandboxStoreError::SequenceConflict,
            ));
        }
        encoded_bytes = encoded_bytes
            .checked_add(document_bytes)
            .filter(|total| *total <= PHYSICAL_AGENT_LOG_BYTES)
            .ok_or_else(|| {
                tracing::warn!(
                    limit_kind = "physical_bytes",
                    scanned_entries = entry_count,
                    scanned_bytes = encoded_bytes,
                    document_bytes,
                    max_bytes = PHYSICAL_AGENT_LOG_BYTES,
                    "Agent log capacity rejected"
                );
                capacity_error()
            })
            .map_err(TransactionLogError::Domain)?;
        visit(entry).map_err(TransactionLogError::Domain)?;
        entry_count = expected.unwrap_or_default();
    }
    if entry_count != last_seq {
        return Err(TransactionLogError::Domain(
            SandboxStoreError::SequenceConflict,
        ));
    }
    observation.complete();
    Ok(LogSummary {
        entry_count,
        encoded_bytes,
    })
}

pub(super) fn ensure_append_capacity(
    complete: &LogSummary,
    appended: &[AgentLogEntry],
    capacity: AppendCapacity,
) -> Result<(), SandboxStoreError> {
    if appended.is_empty() {
        return Ok(());
    }
    ensure_append_batch_size(appended.len())?;
    let pending_count = u64::try_from(appended.len()).map_err(|_| capacity_error())?;
    let mut pending_bytes = 0_usize;
    for entry in appended {
        let document = log_document(entry)?;
        let document_bytes = encoded_document_len(&document)?;
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

pub(super) fn ensure_append_batch_size(pending: usize) -> Result<(), SandboxStoreError> {
    if pending <= MAX_AGENT_LOG_BATCH_SIZE {
        Ok(())
    } else {
        tracing::warn!(
            limit_kind = "batch_entries",
            pending_entries = pending,
            max_entries = MAX_AGENT_LOG_BATCH_SIZE,
            "Agent log capacity rejected"
        );
        Err(capacity_error())
    }
}

fn encoded_document_len(document: &Document) -> Result<usize, SandboxStoreError> {
    document
        .to_vec()
        .map(|encoded| encoded.len())
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
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
mod tests {
    use super::*;

    fn entry(seq: u64) -> AgentLogEntry {
        AgentLogEntry {
            agent_id: AgentId("agt_capacity".into()),
            seq,
            appended_at: "2026-09-05T00:00:00Z".into(),
            kind: layer2_core::sandbox::AgentLogKind::Deleted,
        }
    }

    #[test]
    fn normal_and_terminal_agent_log_capacity_are_distinct_and_bounded() {
        assert!(capacity_allows(
            NORMAL_AGENT_LOG_ENTRIES - 1,
            NORMAL_AGENT_LOG_BYTES - 1,
            1,
            1,
            AppendCapacity::Normal,
        ));
        assert!(!capacity_allows(
            NORMAL_AGENT_LOG_ENTRIES,
            NORMAL_AGENT_LOG_BYTES,
            1,
            1,
            AppendCapacity::Normal,
        ));
        assert!(capacity_allows(
            NORMAL_AGENT_LOG_ENTRIES,
            NORMAL_AGENT_LOG_BYTES,
            STOP_RESERVE_ENTRIES,
            STOP_RESERVE_BYTES,
            AppendCapacity::StopOrFailure,
        ));
        assert!(capacity_allows(
            NORMAL_AGENT_LOG_ENTRIES,
            NORMAL_AGENT_LOG_BYTES,
            STOP_RESERVE_ENTRIES + DELETE_RESERVE_ENTRIES,
            STOP_RESERVE_BYTES + DELETE_RESERVE_BYTES,
            AppendCapacity::Delete,
        ));
        assert!(!capacity_allows(
            STOP_AGENT_LOG_ENTRIES,
            STOP_AGENT_LOG_BYTES,
            DELETE_RESERVE_ENTRIES + 1,
            DELETE_RESERVE_BYTES,
            AppendCapacity::Delete,
        ));
    }

    #[test]
    fn append_checks_leave_staged_terminal_headroom() {
        let normal_full = LogSummary {
            entry_count: NORMAL_AGENT_LOG_ENTRIES,
            encoded_bytes: NORMAL_AGENT_LOG_BYTES,
        };
        assert_eq!(
            ensure_append_capacity(&normal_full, &[entry(10_001)], AppendCapacity::Normal),
            Err(SandboxStoreError::AgentLogCapacityExceeded)
        );
        assert!(ensure_append_capacity(
            &normal_full,
            &[entry(10_001)],
            AppendCapacity::StopOrFailure,
        )
        .is_ok());
        assert!(ensure_append_capacity(&normal_full, &[], AppendCapacity::Normal).is_ok());

        let stop_full = LogSummary {
            entry_count: STOP_AGENT_LOG_ENTRIES,
            encoded_bytes: STOP_AGENT_LOG_BYTES,
        };
        assert!(
            ensure_append_capacity(&stop_full, &[entry(10_065)], AppendCapacity::Delete).is_ok()
        );
        assert_eq!(
            ensure_append_capacity(&stop_full, &[entry(10_065)], AppendCapacity::StopOrFailure,),
            Err(SandboxStoreError::AgentLogCapacityExceeded)
        );
    }

    #[test]
    fn atomic_append_batches_are_bounded_before_materialization() {
        assert!(ensure_append_batch_size(0).is_ok());
        assert!(ensure_append_batch_size(MAX_AGENT_LOG_BATCH_SIZE).is_ok());
        assert_eq!(
            ensure_append_batch_size(MAX_AGENT_LOG_BATCH_SIZE + 1),
            Err(SandboxStoreError::AgentLogCapacityExceeded)
        );
    }
}
