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
    /// Carry the persistence adapter's publication context into dependent stores.
    /// The captured claim and monotonic validity remain in WorkAuthority.
    async fn scope_workspace_publication<'a>(
        &'a self,
        work: std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<bool, SandboxStoreError>> + Send + 'a>,
        >,
    ) -> Result<bool, SandboxStoreError> {
        work.await
    }

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
    /// Keep the original holder/fence and revision; expired authority cannot be
    /// renewed. Durable implementations use server time, not the caller hint.
    async fn renew_workspace_work(
        &self,
        claimed: &Workspace,
        now: &str,
        requested_until: &str,
    ) -> Result<Option<String>, SandboxStoreError>;
    async fn renew_workspace_authority(
        &self,
        _claimed: &Workspace,
        _now: &str,
        _requested_until: &str,
    ) -> Result<Option<super::WorkLeaseGrant>, SandboxStoreError> {
        Err(SandboxStoreError::Conflict)
    }
}
