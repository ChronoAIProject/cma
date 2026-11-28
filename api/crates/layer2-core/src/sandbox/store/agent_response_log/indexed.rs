//! Certified output ordering and original source reads within one observation.
use super::SandboxStoreError;
use crate::{sandbox::*, AgentId};

pub const OUTPUT_PAGE_CHANGES: u16 = 8;
pub const OUTPUT_PAGE_POINTS: usize = 32;
pub const OUTPUT_PAGE_BYTES: usize = 4 * 1024 * 1024;

/// Scalar facts only. Payloads are obtained from their original source points.
#[derive(Clone, serde::Serialize)]
pub struct AgentOutputTurn {
    pub facts: AgentLogTurnFacts,
    pub legacy_model_known: bool,
    pub usage_sequence: Option<u64>,
    pub terminal_sequence: Option<u64>,
    pub prewrite_sequence: Option<u64>,
}

#[derive(Clone, serde::Serialize)]
pub struct AgentOutputMetadata {
    pub agent_id: AgentId,
    pub through: u64,
    pub created_at: String,
    pub runtime_generation: Option<u64>,
    pub active_turn: Option<u64>,
    pub initial_model: Option<String>,
    pub turn: Option<AgentOutputTurn>,
    pub index: Option<agent_output_index::AgentTurnOutputIndex>,
}

/// A read-only snapshot. Implementations must reject absent, inconsistent or
/// stale certified evidence; None is not a segmented fallback signal.
#[async_trait::async_trait]
pub trait AgentOutputReader: Send {
    fn metadata(&self) -> &AgentOutputMetadata;
    async fn changes(
        &mut self,
        from: u64,
        limit: u16,
    ) -> Result<Vec<agent_output_index::AgentOutputChange>, SandboxStoreError>;
    async fn tool(
        &mut self,
        call: &str,
    ) -> Result<agent_output_index::AgentToolOutputState, SandboxStoreError>;
    async fn points(&mut self, sequences: &[u64]) -> Result<Vec<AgentLogEntry>, SandboxStoreError>;
    async fn open_message(&mut self) -> Result<String, SandboxStoreError> {
        Err(SandboxStoreError::Corrupt(
            "indexed open message is missing its checkpoint".into(),
        ))
    }
    /// A bounded consecutive source page for consumers of every durable update.
    async fn source_page(
        &mut self,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError>;
}
