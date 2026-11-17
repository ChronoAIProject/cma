//! Runtime-owned Agent history export boundary.

use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentHistoryExportError {
    Unavailable,
    Capacity,
    Unsupported,
    Rejected,
}

#[async_trait]
pub trait AgentHistoryExporter: Send + Sync {
    async fn export(
        &self,
        runtime_id: &str,
        agent_id: &str,
    ) -> Result<Vec<u8>, AgentHistoryExportError>;
}
