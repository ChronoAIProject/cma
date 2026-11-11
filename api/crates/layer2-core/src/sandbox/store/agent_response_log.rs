//! Optional, proven source windows for command-bound response observations.

use super::{Agent, AgentLogEntry, SandboxStoreError};

/// A complete target-turn interval, ending at the captured head or including
/// the next TurnStarted as an immutable end witness. No preceding lifetime
/// prefix is needed when the first turn retains its execution model.
pub struct AgentResponseLogWindow {
    pub through: u64,
    pub turn_sequence: Option<u64>,
    pub entries: Vec<AgentLogEntry>,
}

#[async_trait::async_trait]
pub trait AgentResponseLogStore: Send + Sync {
    /// Reads only. The proof, head and rows share one coherent snapshot.
    /// None permits legacy fallback for an unsupported/budget-exceeding
    /// interval. Corruption/quarantine is an error. A segmented implementation
    /// must never return None to authorize a legacy lifetime read.
    async fn agent_response_log_window(
        &self,
        agent: &Agent,
        turn_sequence: Option<u64>,
    ) -> Result<Option<AgentResponseLogWindow>, SandboxStoreError>;
}
