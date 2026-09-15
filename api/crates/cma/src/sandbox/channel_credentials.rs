//! Provenance gate for ordinary lifecycle credentials. Every effect receives
//! its concrete Sandbox so origin can be checked before owner-token lookup.
//! Channel lifecycle requests remain held until their dedicated human-authority
//! path supports that lifecycle phase; no owner-subject token fallback exists.
use super::{CredentialError, CredentialMaterial, SandboxCredentialController};
use layer2_core::{sandbox::Sandbox, SandboxId};
use std::{sync::Arc, time::SystemTime};

#[async_trait::async_trait]
pub(crate) trait SandboxCredentialProvenance: Send + Sync {
    async fn require_ordinary(&self, sandbox_id: &SandboxId) -> Result<(), CredentialError>;
}
pub(crate) struct ChannelGuardedCredentials {
    ordinary: Arc<dyn SandboxCredentialController>,
    provenance: Arc<dyn SandboxCredentialProvenance>,
    lifecycle: Option<Arc<dyn super::channel_lifecycle_credentials::ChannelLifecycleCredentials>>,
}
impl ChannelGuardedCredentials {
    #[cfg(test)]
    pub fn new(
        ordinary: Arc<dyn SandboxCredentialController>,
        provenance: Arc<dyn SandboxCredentialProvenance>,
    ) -> Arc<Self> {
        Arc::new(Self {
            ordinary,
            provenance,
            lifecycle: None,
        })
    }
    pub fn with_lifecycle(
        ordinary: Arc<dyn SandboxCredentialController>,
        provenance: Arc<dyn SandboxCredentialProvenance>,
        lifecycle: Option<
            Arc<dyn super::channel_lifecycle_credentials::ChannelLifecycleCredentials>,
        >,
    ) -> Arc<Self> {
        Arc::new(Self {
            ordinary,
            provenance,
            lifecycle,
        })
    }
    fn lifecycle(
        &self,
    ) -> Result<
        &Arc<dyn super::channel_lifecycle_credentials::ChannelLifecycleCredentials>,
        CredentialError,
    > {
        self.lifecycle.as_ref().ok_or_else(|| {
            CredentialError::Unavailable("Channel lifecycle authority is not available".into())
        })
    }
}
#[async_trait::async_trait]
impl SandboxCredentialController for ChannelGuardedCredentials {
    async fn commit_channel_phase(
        &self,
        sandbox: &Sandbox,
        mutation: &layer2_core::sandbox::store::SandboxLifecycleMutation,
    ) -> Result<bool, CredentialError> {
        match self.provenance.require_ordinary(&sandbox.id).await {
            Ok(()) => self.ordinary.commit_channel_phase(sandbox, mutation).await,
            Err(_) => {
                self.lifecycle()?.commit_phase(sandbox, mutation).await?;
                Ok(true)
            }
        }
    }
    async fn require_lifecycle_restore(&self, sandbox: &Sandbox) -> Result<(), CredentialError> {
        match self.provenance.require_ordinary(&sandbox.id).await {
            Ok(()) => self.ordinary.require_lifecycle_restore(sandbox).await,
            Err(_) => self.lifecycle()?.require_restore(sandbox).await,
        }
    }
    async fn restore_channel_credential(&self, sandbox: &Sandbox) -> Result<bool, CredentialError> {
        match self.provenance.require_ordinary(&sandbox.id).await {
            Ok(()) => self.ordinary.restore_channel_credential(sandbox).await,
            Err(_) => {
                self.lifecycle()?.restore_credential(sandbox).await?;
                Ok(true)
            }
        }
    }
    async fn publish_channel_active(&self, sandbox: &Sandbox) -> Result<bool, CredentialError> {
        match self.provenance.require_ordinary(&sandbox.id).await {
            Ok(()) => self.ordinary.publish_channel_active(sandbox).await,
            Err(_) => {
                self.lifecycle()?.publish_active(sandbox).await?;
                Ok(true)
            }
        }
    }
    async fn issue(
        &self,
        sandbox: &Sandbox,
        generation: u64,
        now: SystemTime,
    ) -> Result<CredentialMaterial, CredentialError> {
        self.provenance.require_ordinary(&sandbox.id).await?;
        self.ordinary.issue(sandbox, generation, now).await
    }
    async fn revoke(
        &self,
        sandbox: &Sandbox,
        provider_key_id: &str,
    ) -> Result<(), CredentialError> {
        self.provenance.require_ordinary(&sandbox.id).await?;
        self.ordinary.revoke(sandbox, provider_key_id).await
    }
    async fn settle_revocations(&self, sandbox: &Sandbox) -> Result<(), CredentialError> {
        self.provenance.require_ordinary(&sandbox.id).await?;
        self.ordinary.settle_revocations(sandbox).await
    }
    async fn material(
        &self,
        sandbox_id: &SandboxId,
        generation: u64,
    ) -> Result<Option<CredentialMaterial>, CredentialError> {
        self.ordinary.material(sandbox_id, generation).await
    }
    async fn custodied(
        &self,
        sandbox_id: &SandboxId,
    ) -> Result<Vec<CredentialMaterial>, CredentialError> {
        self.ordinary.custodied(sandbox_id).await
    }
    async fn forget(&self, sandbox_id: &SandboxId, generation: u64) -> Result<(), CredentialError> {
        self.provenance.require_ordinary(sandbox_id).await?;
        self.ordinary.forget(sandbox_id, generation).await
    }
}
