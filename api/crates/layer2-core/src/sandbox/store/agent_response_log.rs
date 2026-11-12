//! Optional, proven source windows for command-bound response observations.

use super::{Agent, AgentLogEntry, SandboxStoreError};

pub mod indexed;

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
    /// Routing hint only; the subsequent observation still proves the source.
    async fn agent_output_available(&self, _agent: &Agent) -> Result<bool, SandboxStoreError> {
        Ok(false)
    }
    /// None means a proven legacy format. The caller may retain its legacy
    /// implementation; an adopted format must return a reader or an error.
    async fn agent_output_reader(
        &self,
        _agent: &Agent,
        _turn: Option<u64>,
    ) -> Result<Option<Box<dyn indexed::AgentOutputReader + '_>>, SandboxStoreError> {
        Ok(None)
    }

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
