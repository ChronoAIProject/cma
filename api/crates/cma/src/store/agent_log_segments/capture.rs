//! Read-only captured source. Unsupported contexts refuse before any write.
use layer2_core::sandbox::{
    store::SandboxStoreError, Agent, AgentLogEntry, AgentLogProjectionState,
};
use mongodb::{
    bson::{doc, Binary, Document, Timestamp},
    options::ReadConcern,
    ClientSession,
};

mod snapshot;

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
    context: SnapshotContext,
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

// The public driver cannot identify successive transactions on one session.
// Transaction mode remains closed until the shared transaction-scope contract
// is amended; a session ID alone must never authorize publication.
struct SnapshotContext {
    session_id: Document,
    at: Timestamp,
    database: String,
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
    if super::read_format(&row)?.is_some() {
        // Certified generation maxima and the registered projection reader
        // are required before segmented capture can establish absence facts.
        return Err(pending());
    }
    let context = SnapshotContext {
        session_id: session.id().clone(),
        at,
        database: store.agents.namespace().db,
    };
    snapshot::legacy(store, current, &row, session, context).await
}

pub(in crate::store) async fn points(
    store: &MongoSandboxStore,
    captured: &CapturedLog,
    sequences: &[u64],
    session: &mut ClientSession,
    mode: CaptureMode,
) -> Result<Vec<AgentLogEntry>, TransactionLogError> {
    captured.validate_snapshot(store, session, mode)?;
    if sequences.is_empty()
        || sequences.len() > 32
        || sequences
            .iter()
            .any(|seq| *seq == 0 || *seq > captured.through())
        || sequences.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(SandboxStoreError::InvalidPage.into());
    }
    snapshot::points(store, captured, sequences, session).await
}

impl CapturedLog {
    fn validate_snapshot(
        &self,
        store: &MongoSandboxStore,
        session: &ClientSession,
        mode: CaptureMode,
    ) -> Result<(), TransactionLogError> {
        if mode != CaptureMode::Snapshot
            || session.id() != &self.context.session_id
            || session.snapshot_time() != Some(self.context.at)
            || store.agents.namespace().db != self.context.database
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
