use async_trait::async_trait;

use crate::ids::WorkspaceId;

use super::{Sandbox, SandboxStoreError, Workspace};

/// Exact receipts for a Workspace whose preparation completed before admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePreparationCompletion {
    pub prepare: layer3_adapter::WorkspaceReceipt,
    pub setup: layer3_adapter::SetupReceipt,
}

/// Durable workspace lifecycle discovery and lease fencing.
#[async_trait]
pub trait WorkspaceWorkerStore: Send + Sync {
    /// Exact durable preparation evidence for this admitted Workspace and current parent.
    /// The worker must independently inspect that runtime before publishing Ready.
    async fn prepared_workspace_completion(
        &self,
        _sandbox: &Sandbox,
        _workspace: &Workspace,
    ) -> Result<Option<WorkspacePreparationCompletion>, SandboxStoreError> {
        Ok(None)
    }

    async fn due_workspace_work(
        &self,
        now: &str,
        limit: u16,
    ) -> Result<Vec<WorkspaceId>, SandboxStoreError>;
    async fn claim_workspace_work(
        &self,
        id: &WorkspaceId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Result<Option<Workspace>, SandboxStoreError>;
}
