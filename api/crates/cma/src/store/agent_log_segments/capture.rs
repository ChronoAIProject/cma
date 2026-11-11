//! Frozen source-proof port. Unsupported captures refuse before any write.
//!
//! The opaque proof cannot be constructed by callers. The executable capture
//! will replace these refusal bodies after projection and snapshot integration;
//! this signature checkpoint cannot authorize publication or recovery.
use layer2_core::sandbox::{
    store::SandboxStoreError, Agent, AgentLogEntry, AgentLogProjectionState,
};
use mongodb::{bson::Binary, ClientSession};

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

pub(in crate::store) async fn capture(
    _store: &MongoSandboxStore,
    _current: &Agent,
    _session: &mut ClientSession,
    _mode: CaptureMode,
) -> Result<CapturedLog, TransactionLogError> {
    Err(pending())
}

pub(in crate::store) async fn points(
    _store: &MongoSandboxStore,
    _captured: &CapturedLog,
    _sequences: &[u64],
    _session: &mut ClientSession,
    _mode: CaptureMode,
) -> Result<Vec<AgentLogEntry>, TransactionLogError> {
    Err(pending())
}

fn pending() -> TransactionLogError {
    SandboxStoreError::Unavailable(
        "certified Agent log capture is not available in this reader checkpoint".into(),
    )
    .into()
}
