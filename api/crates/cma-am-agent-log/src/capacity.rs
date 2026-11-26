//! Bounded Agent-log append capacity and reserve classes.
use layer2_core::sandbox::store::{SandboxStoreError, MAX_AGENT_LOG_BATCH_SIZE};

pub use layer2_core::sandbox::store::NORMAL_AGENT_LOG_ENTRIES;
pub const NORMAL_AGENT_LOG_BYTES: usize = 64 * 1024 * 1024;
pub const STOP_RESERVE_ENTRIES: u64 = 64;
pub const STOP_RESERVE_BYTES: usize = 512 * 1024;
pub const DELETE_RESERVE_ENTRIES: u64 = 64;
pub const DELETE_RESERVE_BYTES: usize = 512 * 1024;
pub const STOP_AGENT_LOG_ENTRIES: u64 = NORMAL_AGENT_LOG_ENTRIES + STOP_RESERVE_ENTRIES;
pub const STOP_AGENT_LOG_BYTES: usize = NORMAL_AGENT_LOG_BYTES + STOP_RESERVE_BYTES;
pub const PHYSICAL_AGENT_LOG_ENTRIES: u64 = STOP_AGENT_LOG_ENTRIES + DELETE_RESERVE_ENTRIES;
pub const PHYSICAL_AGENT_LOG_BYTES: usize = STOP_AGENT_LOG_BYTES + DELETE_RESERVE_BYTES;
pub const MAX_AGENT_LOG_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppendCapacity {
    Normal,
    StopOrFailure,
    Delete,
}

impl AppendCapacity {
    pub fn limits(self) -> (u64, usize) {
        match self {
            Self::Normal => (NORMAL_AGENT_LOG_ENTRIES, NORMAL_AGENT_LOG_BYTES),
            Self::StopOrFailure => (STOP_AGENT_LOG_ENTRIES, STOP_AGENT_LOG_BYTES),
            Self::Delete => (PHYSICAL_AGENT_LOG_ENTRIES, PHYSICAL_AGENT_LOG_BYTES),
        }
    }
}

pub fn ensure_append_batch_size(pending: usize) -> Result<(), SandboxStoreError> {
    if pending <= MAX_AGENT_LOG_BATCH_SIZE {
        Ok(())
    } else {
        tracing::warn!(
            limit_kind = "batch_entries",
            pending_entries = pending,
            max_entries = MAX_AGENT_LOG_BATCH_SIZE,
            "Agent log capacity rejected"
        );
        Err(SandboxStoreError::AgentLogCapacityExceeded)
    }
}
