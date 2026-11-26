use async_trait::async_trait;

use super::{SandboxEvent, SandboxEventKind, SandboxId, SandboxStoreError};

#[async_trait]
pub trait SandboxEventStore: Send + Sync {
    /// The first failed lifecycle attempt since the last successful phase
    /// transition of the current operation. Older histories can lack a receipt.
    async fn lifecycle_retry_started_at(
        &self,
        _sandbox_id: &SandboxId,
        _admitted_at: &str,
    ) -> Result<Option<String>, SandboxStoreError> {
        Ok(None)
    }

    async fn append_sandbox_event(
        &self,
        sandbox_id: &SandboxId,
        appended_at: String,
        kind: SandboxEventKind,
    ) -> Result<SandboxEvent, SandboxStoreError>;
    async fn sandbox_events(
        &self,
        sandbox_id: &SandboxId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<SandboxEvent>, SandboxStoreError>;
}
