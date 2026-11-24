use async_trait::async_trait;

use super::{Sandbox, SandboxId, SandboxStoreError};

/// Durable lifecycle work discovery, lease fencing and terminal purge.
#[async_trait]
pub trait SandboxWorkerStore: Send + Sync {
    /// Carry the persistence adapter's publication context into dependent stores.
    /// The captured claim and monotonic validity remain in WorkAuthority.
    async fn scope_sandbox_publication<'a>(
        &'a self,
        work: std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<bool, SandboxStoreError>> + Send + 'a>,
        >,
    ) -> Result<bool, SandboxStoreError> {
        work.await
    }

    /// Completed installation attached to this exact ownership and runtime.
    /// The worker must match the current request digest and inspect the live
    /// process before advancing; this proof never authorizes runtime dispatch.
    async fn completed_installation_digest(
        &self,
        _sandbox: &Sandbox,
    ) -> Result<Option<String>, SandboxStoreError> {
        Ok(None)
    }

    /// Return the exact verified native image for a Sandbox bound by canary admission.
    async fn canary_runtime_image(
        &self,
        _sandbox: &Sandbox,
    ) -> Result<Option<layer3_adapter::RuntimeImage>, SandboxStoreError> {
        Ok(None)
    }

    /// Release authority already committed with this Sandbox's creation operation.
    async fn admitted_runtime_release(
        &self,
        _sandbox: &Sandbox,
    ) -> Result<Option<layer3_adapter::ReleasedRuntime>, SandboxStoreError> {
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
    /// Renew the captured work identity without advancing its revision. The
    /// durable store uses its own clock and refuses expired or replaced claims.
    /// `now` supplies the equivalent clock to deterministic in-memory stores.
    async fn renew_sandbox_work(
        &self,
        claimed: &Sandbox,
        now: &str,
        requested_until: &str,
    ) -> Result<Option<String>, SandboxStoreError>;
    /// Short-lease execution requires a store-clock receipt. Implementations
    /// without one must refuse, rather than infer validity from a caller clock.
    async fn renew_sandbox_authority(
        &self,
        _claimed: &Sandbox,
        _now: &str,
        _requested_until: &str,
    ) -> Result<Option<super::WorkLeaseGrant>, SandboxStoreError> {
        Err(SandboxStoreError::Conflict)
    }
    async fn purge_sandbox(
        &self,
        id: &SandboxId,
        expected_revision: u64,
        now: &str,
    ) -> Result<(), SandboxStoreError>;
}
