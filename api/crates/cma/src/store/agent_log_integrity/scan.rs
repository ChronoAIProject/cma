//! Explicit complete audit; all fallbacks keep caller accounting.
use super::*;
use crate::store::{
    cooperative_budget::Policy,
    execution_sections::{Clock, MonotonicClock},
};
use futures_util::TryStreamExt;
use mongodb::options::ReadConcern;
use std::future::IntoFuture;

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
    scan_complete_log(caller, store, agent_id, last_seq, session, false, visit).await
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
    scan_complete_log(caller, store, agent_id, last_seq, session, true, visit).await
}

pub(in crate::store) async fn scan_complete_log<F>(
    caller: ScanCaller,
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    last_seq: u64,
    session: &mut mongodb::ClientSession,
    snapshot: bool,
    mut visit: F,
) -> Result<LogSummary, TransactionLogError>
where
    F: FnMut(AgentLogEntry) -> Result<(), SandboxStoreError>,
{
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
            if matches!(
                &error,
                TransactionLogError::Domain(SandboxStoreError::Corrupt(_))
            ) {
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
    if !visitor_failed
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
            clock: MonotonicClock,
        }
    }
    #[cfg(test)]
    pub(in crate::store) fn with_clock<C: Clock>(self, clock: C) -> ScanOptions<C> {
        ScanOptions {
            snapshot: self.snapshot,
            cooperation: self.cooperation,
            clock,
        }
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
    if last_seq > PHYSICAL_AGENT_LOG_ENTRIES {
        tracing::warn!(
            limit_kind = "physical_entries",
            existing_entries = last_seq,
            max_entries = PHYSICAL_AGENT_LOG_ENTRIES,
            "Agent log capacity rejected"
        );
        return Err(capacity_error().into());
    }
    let mut read = crate::store::mongo_read_telemetry::Read::find(collection.name());
    let raw_log = collection.clone_with_type::<RawDocumentBuf>();
    let find = raw_log
        .find(mongodb::bson::doc! { "agent_id": agent_id.as_str() })
        .sort(mongodb::bson::doc! { "seq": 1 })
        .limit(i64::try_from(PHYSICAL_AGENT_LOG_ENTRIES + 1).unwrap_or(i64::MAX));
    let find = if mode.options.snapshot {
        find.read_concern(ReadConcern::snapshot())
    } else {
        find
    };
    // Track suspension of the initial find too, so network wait never
    // becomes an uninterrupted execution section. Driver-poll timing below
    // is restricted to cursor advancement, where batch copying occurs.
    let mut cursor = observation
        .observe(find.session(&mut *session).into_future(), false)
        .await
        .map_err(TransactionLogError::Driver)?;
    let mut entry_count = 0_u64;
    let mut encoded_bytes = 0_usize;
    let mut head_hash = [0_u8; HASH_BYTES];
    while let Some(raw) = observation
        .observe(cursor.stream(session).try_next(), true)
        .await
        .map_err(TransactionLogError::Driver)?
    {
        let document_bytes = raw.as_bytes().len();
        observation.document(document_bytes);
        read.document(document_bytes);
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
        head_hash = hash_raw(raw.as_bytes());
        entry_count = expected.unwrap_or_default();
        observation.checkpoint().await;
    }
    if entry_count != last_seq {
        return Err(TransactionLogError::Domain(
            SandboxStoreError::SequenceConflict,
        ));
    }
    // A known hash/identity conflict stays a failed complete validation even
    // when the structural traversal alone accepts the changed payload.
    if !mode.known_corrupt {
        observation.complete();
    }
    read.complete();
    Ok(LogSummary {
        entry_count,
        encoded_bytes,
        head_hash,
        semantic: None,
    })
}
