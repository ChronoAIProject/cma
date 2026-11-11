//! Read-only captured source. Unsupported contexts refuse before any write.
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
mod segmented;
use super::transaction_scope::{TransactionIdentity, TransactionScope};

use super::Head;
use crate::store::{agent_log_integrity::TransactionLogError, sandbox_mongo::MongoSandboxStore};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::store) enum CaptureMode {
    Snapshot,
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
    if mode != CaptureMode::Snapshot {
        return Err(pending());
    }
    let row = store
        .agents
        .find_one(doc! {"_id": current.id.as_str()})
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?
        .ok_or_else(super::corrupt)?;
    let at = session.snapshot_time().ok_or_else(pending)?;
    let actual = crate::store::agents::read_agent(&row)?;
    if &actual != current {
        return Err(SandboxStoreError::SequenceConflict.into());
    }
    let context = SourceContext::Snapshot {
        session_id: session.id().clone(),
        at,
        database: store.agents.namespace().db,
    };
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
        segmented::read(store, current, row, session, context, snapshot).await
    } else {
        legacy::read(store, current, row, session, context, snapshot).await
    }
}

impl CapturedLog {
    fn validate_snapshot(
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
