//! Read-only captured source. Unsupported contexts refuse before any write.
use futures_util::TryStreamExt;
use layer2_core::sandbox::{
    store::SandboxStoreError, Agent, AgentLogEntry, AgentLogProjectionState,
};
use mongodb::{
    bson::{doc, Binary, Document, Timestamp},
    options::ReadConcern,
    ClientSession,
};

mod legacy;
mod point_reads;
pub(in crate::store) mod preview_budget;
mod segmented;
mod snapshot;
use super::transaction_scope::{TransactionIdentity, TransactionScope};

use super::Head;
use crate::store::{agent_log_integrity::TransactionLogError, sandbox_mongo::MongoSandboxStore};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::store) enum CaptureMode {
    Snapshot,
    PrivacyExport,
    // Production transaction capture requires the exclusive TransactionScope.
    // Retain the invalid raw-mode token only for refusal qualification tests.
    #[cfg(test)]
    Transaction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::store) enum CapturedFormat {
    Legacy,
    Segmented,
}

pub(in crate::store) struct CapturedLog {
    agent: Agent,
    format: CapturedFormat,
    incarnation: String,
    latest_entry: Option<AgentLogEntry>,
    projection: AgentLogProjectionState,
    max_opened_generation: u64,
    max_retired_generation: u64,
    active_head: Head,
    active_leaf_digests: Binary,
    context: SourceContext,
}

impl CapturedLog {
    pub fn agent(&self) -> &Agent {
        &self.agent
    }
    pub fn format(&self) -> CapturedFormat {
        self.format
    }
    pub fn through(&self) -> u64 {
        self.agent.conversation.last_seq
    }
    pub fn incarnation(&self) -> &str {
        &self.incarnation
    }
    pub fn latest_entry(&self) -> Option<&AgentLogEntry> {
        self.latest_entry.as_ref()
    }
    pub fn projection(&self) -> &AgentLogProjectionState {
        &self.projection
    }
    pub fn max_opened_generation(&self) -> u64 {
        self.max_opened_generation
    }
    pub fn max_retired_generation(&self) -> u64 {
        self.max_retired_generation
    }
    pub fn active_head(&self) -> &Head {
        &self.active_head
    }
    pub fn active_leaf_digests(&self) -> &Binary {
        &self.active_leaf_digests
    }
}

enum SourceContext {
    Snapshot {
        session_id: Document,
        at: Timestamp,
        database: String,
    },
    Transaction {
        identity: TransactionIdentity,
        database: String,
    },
}

pub(in crate::store) async fn capture(
    store: &MongoSandboxStore,
    current: &Agent,
    session: &mut ClientSession,
    mode: CaptureMode,
) -> Result<CapturedLog, TransactionLogError> {
    if !matches!(mode, CaptureMode::Snapshot | CaptureMode::PrivacyExport) {
        return Err(pending());
    }
    let (row, at) = snapshot::agent(store, current, session).await?;
    let actual = crate::store::agents::read_agent(&row)?;
    if &actual != current {
        return Err(SandboxStoreError::SequenceConflict.into());
    }
    let context = SourceContext::Snapshot {
        session_id: session.id().clone(),
        at,
        database: store.agents.namespace().db,
    };
    if mode == CaptureMode::PrivacyExport {
        if super::read_format(&row)?.is_none() {
            return Err(pending());
        }
        return segmented::read(
            store,
            current,
            &row,
            session,
            context,
            true,
            segmented::ReadBudget::Export,
        )
        .await;
    }
    source(store, current, &row, session, context, true).await
}

pub(in crate::store) async fn points(
    store: &MongoSandboxStore,
    captured: &CapturedLog,
    sequences: &[u64],
    session: &mut ClientSession,
    mode: CaptureMode,
) -> Result<Vec<AgentLogEntry>, TransactionLogError> {
    captured.validate_snapshot(store, session, mode)?;
    validate_sequences(captured, sequences)?;
    point_reads::read(store, captured, sequences, session, true).await
}

fn validate_sequences(
    captured: &CapturedLog,
    sequences: &[u64],
) -> Result<(), TransactionLogError> {
    if sequences.is_empty()
        || sequences.len() > 32
        || sequences
            .iter()
            .any(|seq| *seq == 0 || *seq > captured.through())
        || sequences.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(SandboxStoreError::InvalidPage.into());
    }
    Ok(())
}

async fn source(
    store: &MongoSandboxStore,
    current: &Agent,
    row: &Document,
    session: &mut ClientSession,
    context: SourceContext,
    snapshot: bool,
) -> Result<CapturedLog, TransactionLogError> {
    if super::read_format(row)?.is_some() {
        segmented::read(
            store,
            current,
            row,
            session,
            context,
            snapshot,
            segmented::ReadBudget::Standard,
        )
        .await
    } else {
        legacy::read(store, current, row, session, context, snapshot).await
    }
}

impl CapturedLog {
    pub(in crate::store) fn validate_snapshot(
        &self,
        store: &MongoSandboxStore,
        session: &ClientSession,
        mode: CaptureMode,
    ) -> Result<(), TransactionLogError> {
        let SourceContext::Snapshot {
            session_id,
            at,
            database,
        } = &self.context
        else {
            return Err(pending());
        };
        if mode != CaptureMode::Snapshot
            || session.id() != session_id
            || session.snapshot_time() != Some(*at)
            || store.agents.namespace().db != *database
        {
            return Err(pending());
        }

        Ok(())
    }
}

fn pending() -> TransactionLogError {
    SandboxStoreError::Unavailable(
        "certified Agent log capture is not available in this source context".into(),
    )
    .into()
}

/// Only the exclusive scope can establish transaction identity.
pub(in crate::store) async fn capture_transaction(
    store: &MongoSandboxStore,
    current: &Agent,
    scope: &mut TransactionScope<'_>,
) -> Result<CapturedLog, TransactionLogError> {
    let row = scope
        .find_one(&store.agents, doc! {"_id":current.id.as_str()})
        .await
        .map_err(TransactionLogError::Driver)?
        .ok_or_else(super::corrupt)?;
    if crate::store::agents::read_agent(&row)? != *current {
        return Err(SandboxStoreError::SequenceConflict.into());
    }
    let context = SourceContext::Transaction {
        identity: scope.identity(),
        database: store.agents.namespace().db,
    };
    source(store, current, &row, scope.session(), context, false).await
}

/// Size the exact segment that the conversation writer will hash. Keep the
/// source rows on the server until their total fits the caller's proof budget.
pub(in crate::store) async fn stale_turn_segment_bytes(
    store: &MongoSandboxStore,
    id: &layer2_core::AgentId,
    row: &Document,
    remaining: i64,
    scope: &mut TransactionScope<'_>,
) -> Result<i64, TransactionLogError> {
    let head = super::read_head(store, row, scope.session(), false).await?;
    if head.active_count == 0 {
        // The writer would reopen a sealed tail to locate its latest entry.
        return Err(SandboxStoreError::Conflict.into());
    }
    if head.active_bytes > remaining {
        return Ok(head.active_bytes);
    }
    let session = scope.session();
    let mut cursor = store
        .agent_log
        .aggregate([
            doc! {"$match": {"agent_id": id.as_str(), "seq": {
                "$gte": head.active_start_seq, "$lte": head.last_seq,
            }}},
            doc! {"$group": {"_id": null, "bytes": {"$sum": {"$toLong": {"$bsonSize": "$$ROOT"}}}}},
        ])
        .max_time(std::time::Duration::from_secs(1))
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    let totals = cursor
        .stream(session)
        .try_next()
        .await
        .map_err(TransactionLogError::Driver)?
        .ok_or_else(super::corrupt)?;
    let bytes = totals.get_i64("bytes").map_err(|_| super::corrupt())?;
    if bytes != head.active_bytes {
        return Err(super::corrupt().into());
    }
    Ok(bytes)
}

pub(in crate::store) async fn points_transaction(
    store: &MongoSandboxStore,
    captured: &CapturedLog,
    sequences: &[u64],
    scope: &mut TransactionScope<'_>,
) -> Result<Vec<AgentLogEntry>, TransactionLogError> {
    captured.validate_transaction(store, scope)?;
    validate_sequences(captured, sequences)?;
    point_reads::read(store, captured, sequences, scope.session(), false).await
}

impl CapturedLog {
    pub(super) fn validate_transaction(
        &self,
        store: &MongoSandboxStore,
        scope: &TransactionScope<'_>,
    ) -> Result<(), TransactionLogError> {
        let SourceContext::Transaction { identity, database } = &self.context else {
            return Err(pending());
        };
        if store.agents.namespace().db != *database {
            return Err(pending());
        }
        scope.require(identity)?;
        Ok(())
    }
}

pub(in crate::store) async fn preview(
    store: &MongoSandboxStore,
    current: &Agent,
    session: &mut ClientSession,
    budget: &mut preview_budget::PreviewBudget,
) -> Result<Option<CapturedLog>, TransactionLogError> {
    let (row, at) = snapshot::agent_budgeted(store, current, session, Some(budget)).await?;
    if crate::store::agents::read_agent(&row)? != *current {
        return Err(SandboxStoreError::SequenceConflict.into());
    }
    if super::read_format(&row)?.is_none() {
        return Ok(None);
    }
    let context = SourceContext::Snapshot {
        session_id: session.id().clone(),
        at,
        database: store.agents.namespace().db,
    };
    segmented::read(
        store,
        current,
        &row,
        session,
        context,
        true,
        segmented::ReadBudget::Preview(budget),
    )
    .await
    .map(Some)
}

pub(in crate::store) async fn preview_points(
    store: &MongoSandboxStore,
    source: &CapturedLog,
    sequences: &[u64],
    session: &mut ClientSession,
    budget: &mut preview_budget::PreviewBudget,
) -> Result<Vec<AgentLogEntry>, TransactionLogError> {
    source.validate_snapshot(store, session, CaptureMode::Snapshot)?;
    validate_sequences(source, sequences)?;
    point_reads::read_budgeted(store, source, sequences, session, true, Some(budget)).await
}
