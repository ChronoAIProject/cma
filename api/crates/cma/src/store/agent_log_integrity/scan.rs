//! Explicit complete audit; all fallbacks keep caller accounting.
use super::*;
use crate::store::{
    cooperative_budget::Policy,
    execution_sections::{Clock, MonotonicClock},
};
use mongodb::options::ReadConcern;

#[path = "scan_cursor_ingress.rs"]
pub(super) mod cursor;
#[path = "scan_ingress.rs"]
pub(super) mod ingress;
#[path = "scan_rows_ingress.rs"]
pub(super) mod rows;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::store) enum IntegrityFailurePolicy {
    Quarantine,
    ReportOnly,
}

struct CompleteScanOptions {
    snapshot: bool,
    failure_policy: IntegrityFailurePolicy,
}

pub(in crate::store) async fn scan_complete_log_with_session<F>(
    caller: ScanCaller,
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    last_seq: u64,
    session: &mut mongodb::ClientSession,
    visit: F,
) -> Result<LogSummary, TransactionLogError>
where
    F: FnMut(AgentLogEntry) -> Result<(), SandboxStoreError>,
{
    scan_complete_log_with_policy(
        caller,
        store,
        agent_id,
        last_seq,
        session,
        IntegrityFailurePolicy::Quarantine,
        visit,
    )
    .await
}

pub(in crate::store) async fn scan_complete_log_with_policy<F>(
    caller: ScanCaller,
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    last_seq: u64,
    session: &mut mongodb::ClientSession,
    failure_policy: IntegrityFailurePolicy,
    visit: F,
) -> Result<LogSummary, TransactionLogError>
where
    F: FnMut(AgentLogEntry) -> Result<(), SandboxStoreError>,
{
    scan_complete_log(
        caller,
        store,
        agent_id,
        last_seq,
        session,
        CompleteScanOptions {
            snapshot: false,
            failure_policy,
        },
        visit,
    )
    .await
}

pub(in crate::store) async fn scan_complete_log_with_snapshot_session<F>(
    caller: ScanCaller,
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    last_seq: u64,
    session: &mut mongodb::ClientSession,
    visit: F,
) -> Result<LogSummary, TransactionLogError>
where
    F: FnMut(AgentLogEntry) -> Result<(), SandboxStoreError>,
{
    scan_complete_log(
        caller,
        store,
        agent_id,
        last_seq,
        session,
        CompleteScanOptions {
            snapshot: true,
            failure_policy: IntegrityFailurePolicy::Quarantine,
        },
        visit,
    )
    .await
}

async fn scan_complete_log<F>(
    caller: ScanCaller,
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    last_seq: u64,
    session: &mut mongodb::ClientSession,
    options: CompleteScanOptions,
    mut visit: F,
) -> Result<LogSummary, TransactionLogError>
where
    F: FnMut(AgentLogEntry) -> Result<(), SandboxStoreError>,
{
    let CompleteScanOptions {
        snapshot,
        failure_policy,
    } = options;
    let find = store.agents.find_one(doc! { "_id": agent_id.as_str() });
    let find = if snapshot {
        find.read_concern(ReadConcern::snapshot())
    } else {
        find
    };
    let row = find
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    let agent = row
        .as_ref()
        .map(crate::store::agents::read_agent)
        .transpose()
        .map_err(TransactionLogError::Domain)?;
    if let (Some(row), Some(agent)) = (row.as_ref(), agent.as_ref()) {
        if crate::store::agent_log_segments::read_format(row)?.is_some() {
            // Legacy callers must migrate to the format-aware reader. Never
            // audit or quarantine a valid segmented prefix as an oversized v1 log.
            return Err(SandboxStoreError::Unavailable(
                "segmented history requires a bounded reader".into(),
            )
            .into());
        }
        let proof = proof::validate_summary(store, agent, row, caller, session, snapshot).await;
        if let Err(error) = proof {
            if failure_policy == IntegrityFailurePolicy::Quarantine
                && matches!(
                    &error,
                    TransactionLogError::Domain(SandboxStoreError::Corrupt(_))
                )
            {
                let _ = session.abort_transaction().await;
                proof::quarantine_row(store, row).await?;
            }
            return Err(error);
        }
    }
    let mut visitor_failed = false;
    let result = scan_collection(
        caller,
        &store.agent_log,
        agent_id,
        last_seq,
        session,
        ScanOptions::new(snapshot, store.scan_cooperation),
        |entry| {
            if let (
                Some(agent),
                layer2_core::sandbox::AgentLogKind::Created {
                    workspace_id,
                    worktree_id,
                },
            ) = (agent.as_ref(), &entry.kind)
            {
                if entry.seq != 1
                    || workspace_id != &agent.workspace_id
                    || worktree_id != agent.creation_worktree_id()
                {
                    return Err(SandboxStoreError::Corrupt(
                        "Agent creation log lineage does not match".into(),
                    ));
                }
            }
            let result = visit(entry);
            visitor_failed |= result.is_err();
            result
        },
    )
    .await;
    if failure_policy == IntegrityFailurePolicy::Quarantine
        && !visitor_failed
        && matches!(
            &result,
            Err(TransactionLogError::Domain(
                SandboxStoreError::Corrupt(_)
                    | SandboxStoreError::SequenceConflict
                    | SandboxStoreError::AgentLogCapacityExceeded
            ))
        )
    {
        let _ = session.abort_transaction().await;
        if let Some(row) = row.as_ref() {
            proof::quarantine_row(store, row).await?;
        }
    }
    result
}

pub(in crate::store) async fn scan_collection<F, C: Clock>(
    caller: ScanCaller,
    collection: &mongodb::Collection<Document>,
    agent_id: &AgentId,
    last_seq: u64,
    session: &mut mongodb::ClientSession,
    options: ScanOptions<C>,
    visit: F,
) -> Result<LogSummary, TransactionLogError>
where
    F: FnMut(AgentLogEntry) -> Result<(), SandboxStoreError>,
{
    scan_rows(
        caller,
        collection,
        agent_id,
        last_seq,
        session,
        ScanMode {
            options,
            known_corrupt: false,
        },
        visit,
    )
    .await
}

pub(super) async fn scan_corruption(
    caller: ScanCaller,
    collection: &mongodb::Collection<Document>,
    agent_id: &AgentId,
    last_seq: u64,
    session: &mut mongodb::ClientSession,
    options: ScanOptions,
) -> Result<LogSummary, TransactionLogError> {
    scan_rows(
        caller,
        collection,
        agent_id,
        last_seq,
        session,
        ScanMode {
            options,
            known_corrupt: true,
        },
        |_| Ok(()),
    )
    .await
}

pub(in crate::store) struct ScanOptions<C = MonotonicClock> {
    snapshot: bool,
    cooperation: Policy,
    raw_batches: bool,
    clock: C,
}
impl Default for ScanOptions {
    fn default() -> Self {
        Self::new(false, Policy::default())
    }
}
impl ScanOptions {
    pub(in crate::store) fn new(snapshot: bool, cooperation: Policy) -> Self {
        Self {
            snapshot,
            cooperation,
            raw_batches: cooperation.enabled,
            clock: MonotonicClock,
        }
    }
    #[cfg(test)]
    pub(in crate::store) fn with_clock<C: Clock>(self, clock: C) -> ScanOptions<C> {
        ScanOptions {
            snapshot: self.snapshot,
            cooperation: self.cooperation,
            raw_batches: self.raw_batches,
            clock,
        }
    }
}
impl<C> ScanOptions<C> {
    // Retain the old cooperative delivery as a paired benchmark control.
    #[cfg(test)]
    pub(in crate::store) fn with_raw_batches(mut self, enabled: bool) -> Self {
        self.raw_batches = enabled;
        self
    }
}
struct ScanMode<C> {
    options: ScanOptions<C>,
    known_corrupt: bool,
}
impl MongoSandboxStore {
    pub(crate) fn with_scan_cooperation(mut self, enabled: bool) -> Self {
        self.scan_cooperation = Policy::enabled(enabled);
        self
    }
}

async fn scan_rows<F, C: Clock>(
    caller: ScanCaller,
    collection: &mongodb::Collection<Document>,
    agent_id: &AgentId,
    last_seq: u64,
    session: &mut mongodb::ClientSession,
    mode: ScanMode<C>,
    mut visit: F,
) -> Result<LogSummary, TransactionLogError>
where
    F: FnMut(AgentLogEntry) -> Result<(), SandboxStoreError>,
{
    let mut observation = crate::store::agent_log_telemetry::Scan::with_clock(
        caller,
        mode.options.cooperation,
        mode.options.clock,
    );
    rows::require_head(last_seq).map_err(TransactionLogError::Domain)?;
    let mut read = crate::store::mongo_read_telemetry::Read::find(collection.name());
    let mut state = rows::ScanState::default();
    // Keep row validation independent of cursor delivery so every path retains
    // the same sequence, capacity, hash and visitor failure boundaries.
    let mut consume = |raw: &mongodb::bson::RawDocument,
                       observation: &mut crate::store::agent_log_telemetry::Scan<C>|
     -> Result<(), TransactionLogError> {
        let document_bytes = raw.as_bytes().len();
        observation.document(document_bytes);
        read.document(document_bytes);
        let document = Document::try_from(raw).map_err(|error| {
            TransactionLogError::Domain(SandboxStoreError::Corrupt(error.to_string()))
        })?;
        let entry = read_entry(&document).map_err(TransactionLogError::Domain)?;
        let expected = state
            .prepare(entry.seq, last_seq, document_bytes)
            .map_err(TransactionLogError::Domain)?;
        visit(entry).map_err(TransactionLogError::Domain)?;
        state.advance(expected, raw);
        Ok(())
    };
    cursor::visit_raw_rows(
        cursor::ScanInput {
            collection,
            agent_id,
            last_seq,
        },
        session,
        mode.options.snapshot,
        mode.options.raw_batches,
        &mut observation,
        &mut consume,
        TransactionLogError::Driver,
    )
    .await?;
    let summary = state
        .finish(last_seq)
        .map_err(TransactionLogError::Domain)?;
    // A known hash/identity conflict stays a failed complete validation even
    // when the structural traversal alone accepts the changed payload.
    if !mode.known_corrupt {
        observation.complete();
    }
    read.complete();
    Ok(summary)
}

fn batch_document(
    item: Result<mongodb::bson::RawBsonRef<'_>, mongodb::bson::error::Error>,
) -> Result<&mongodb::bson::RawDocument, mongodb::error::Error> {
    let item = item.map_err(mongodb::error::Error::from)?;
    // A malformed wire batch is a driver failure, not stored-history corruption
    // eligible for quarantine. Stored row validation still happens in consume.
    item.as_document()
        .ok_or_else(|| mongodb::error::Error::custom("invalid document in cursor batch"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::RawBsonRef;

    #[test]
    fn batch_documents_borrow_rows_and_classify_invalid_wire_items_as_driver_errors() {
        let raw = RawDocumentBuf::from_bytes(doc! { "seq": 1 }.to_vec().unwrap()).unwrap();
        let borrowed = batch_document(Ok(RawBsonRef::Document(&raw))).unwrap();
        assert!(std::ptr::eq(borrowed, raw.as_ref()));
        let decode_error =
            <mongodb::bson::error::Error as serde::de::Error>::custom("malformed wire item");
        for item in [Ok(RawBsonRef::Null), Err(decode_error)] {
            assert!(matches!(
                batch_document(item).map_err(TransactionLogError::Driver),
                Err(TransactionLogError::Driver(_))
            ));
        }
    }
}
