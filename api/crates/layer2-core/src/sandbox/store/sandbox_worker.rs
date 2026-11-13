use async_trait::async_trait;

use super::{Sandbox, SandboxId, SandboxStoreError};

/// Durable lifecycle work discovery, lease fencing and terminal purge.
#[async_trait]
pub trait SandboxWorkerStore: Send + Sync {
    /// Completed installation attached to this exact ownership and runtime.
    /// The worker must match the current request digest and inspect the live
    /// process before advancing; this proof never authorizes runtime dispatch.
    async fn completed_installation_digest(
        &self,
        _sandbox: &Sandbox,
    ) -> Result<Option<String>, SandboxStoreError> {
        Ok(None)
    }

    async fn due_sandbox_work(
        &self,
        now: &str,
        limit: u16,
    ) -> Result<Vec<SandboxId>, SandboxStoreError>;
    async fn claim_sandbox_work(
        &self,
        id: &SandboxId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Result<Option<Sandbox>, SandboxStoreError>;
    async fn purge_sandbox(
        &self,
        id: &SandboxId,
        expected_revision: u64,
        now: &str,
    ) -> Result<(), SandboxStoreError>;
}

