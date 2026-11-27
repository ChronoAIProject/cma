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
pub(crate) struct LegacySandboxCredentials {
    ordinary: Arc<dyn SandboxCredentialController>,
    provenance: Arc<dyn SandboxCredentialProvenance>,
}
impl LegacySandboxCredentials {
    pub fn new(
        ordinary: Arc<dyn SandboxCredentialController>,
        provenance: Arc<dyn SandboxCredentialProvenance>,
    ) -> Arc<Self> {
        Arc::new(Self {
            ordinary,
            provenance,
        })
    }
}
#[async_trait::async_trait]
impl SandboxCredentialController for LegacySandboxCredentials {
    async fn retire_channel(
        &self,
        sandbox: &Sandbox,
        _agents: &dyn super::worker::LiveAgentStopper,
        _now: SystemTime,
    ) -> Result<bool, CredentialError> {
        self.provenance.require_ordinary(&sandbox.id).await?;
        Ok(false)
    }
    async fn require_lifecycle_restore(&self, sandbox: &Sandbox) -> Result<(), CredentialError> {
        self.provenance.require_ordinary(&sandbox.id).await
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
    async fn retirement_authority_version(
        &self,
        sandbox: &Sandbox,
    ) -> Result<Option<layer2_core::sandbox::RetirementAuthorityVersion>, CredentialError> {
        self.provenance.require_ordinary(&sandbox.id).await?;
        self.ordinary.retirement_authority_version(sandbox).await
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
