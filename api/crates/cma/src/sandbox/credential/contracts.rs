use crate::auth::{persistence::SecretValue, AuthState};
use layer2_core::{
    sandbox::{Sandbox, SandboxCredentialAuthority},
    SandboxId, ServiceId, SubjectId,
};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub(crate) struct CredentialMaterial {
    pub source: layer2_core::sandbox::SandboxCredentialSource,
    pub sandbox_id: SandboxId,
    pub owner: SubjectId,
    pub authority: SandboxCredentialAuthority,
    pub value: SecretValue,
}

impl CredentialMaterial {
    pub(crate) fn matches_source(&self) -> bool {
        use layer2_core::sandbox::SandboxCredentialSource;
        use sha2::{Digest, Sha256};
        self.source.accepts(&self.authority)
            && match &self.source {
                SandboxCredentialSource::Managed => true,
                SandboxCredentialSource::Binding { .. } => {
                    format!(
                        "provided-{:x}",
                        Sha256::digest(self.value.expose().as_bytes())
                    ) == self.authority.provider_key_id
                }
                SandboxCredentialSource::Provided { key_sha256 } => {
                    &format!("{:x}", Sha256::digest(self.value.expose().as_bytes())) == key_sha256
                }
            }
    }
}

/// Non-secret outbox entry for a provider key which lost a custody race.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CredentialRevocation {
    pub sandbox_id: SandboxId,
    pub owner: SubjectId,
    pub provider_key_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum CredentialCustodyError {
    #[error("credential generation already exists")]
    Duplicate,
    #[error("credential custody unavailable: {0}")]
    Unavailable(String),
    #[error("stored credential custody is not usable: {0}")]
    Corrupt(String),
}

#[async_trait::async_trait]
pub(crate) trait SandboxCredentialCustody: Send + Sync {
    async fn insert(&self, material: CredentialMaterial) -> Result<(), CredentialCustodyError>;
    async fn get(
        &self,
        sandbox_id: &SandboxId,
        generation: u64,
    ) -> Result<Option<CredentialMaterial>, CredentialCustodyError>;
    async fn list(
        &self,
        sandbox_id: &SandboxId,
    ) -> Result<Vec<CredentialMaterial>, CredentialCustodyError>;
    async fn delete(
        &self,
        sandbox_id: &SandboxId,
        generation: u64,
    ) -> Result<(), CredentialCustodyError>;
    async fn queue_revocation(
        &self,
        revocation: CredentialRevocation,
    ) -> Result<(), CredentialCustodyError>;
    async fn revocations(
        &self,
        sandbox_id: &SandboxId,
    ) -> Result<Vec<CredentialRevocation>, CredentialCustodyError>;
    async fn delete_revocation(
        &self,
        sandbox_id: &SandboxId,
        provider_key_id: &str,
    ) -> Result<(), CredentialCustodyError>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum CredentialError {
    #[error("service is not granted: {0}")]
    ServiceNotGranted(ServiceId),
    #[error("credential dependency unavailable: {0}")]
    Unavailable(String),
    #[error("credential authority was refused: {0}")]
    Security(String),
}

#[async_trait::async_trait]
pub(crate) trait OwnerTokenSource: Send + Sync {
    async fn token_for(&self, owner: &SubjectId) -> Result<Option<SecretValue>, String>;
}

#[async_trait::async_trait]
impl OwnerTokenSource for AuthState {
    async fn token_for(&self, owner: &SubjectId) -> Result<Option<SecretValue>, String> {
        self.delegated_credential_for(owner)
            .await
            .map_err(|error| error.to_string())
    }
}

#[async_trait::async_trait]
pub(crate) trait SandboxCredentialController: Send + Sync {
    async fn retire_channel(
        &self,
        _sandbox: &Sandbox,
        _agents: &dyn crate::sandbox::worker::LiveAgentStopper,
        _now: SystemTime,
    ) -> Result<bool, CredentialError> {
        Ok(false)
    }
    /// Check authority before any phase that will restore runtime access.
    /// Ordinary credentials retain their existing lifecycle authorization.
    async fn require_lifecycle_restore(&self, _sandbox: &Sandbox) -> Result<(), CredentialError> {
        Ok(())
    }
    async fn commit_channel_phase(
        &self,
        _sandbox: &Sandbox,
        _mutation: &layer2_core::sandbox::store::SandboxLifecycleMutation,
    ) -> Result<bool, CredentialError> {
        Ok(false)
    }
    async fn restore_channel_credential(
        &self,
        _sandbox: &Sandbox,
    ) -> Result<bool, CredentialError> {
        Ok(false)
    }
    async fn publish_channel_active(&self, _sandbox: &Sandbox) -> Result<bool, CredentialError> {
        Ok(false)
    }
    async fn issue(
        &self,
        sandbox: &Sandbox,
        generation: u64,
        now: SystemTime,
    ) -> Result<CredentialMaterial, CredentialError>;
    async fn material(
        &self,
        sandbox_id: &SandboxId,
        generation: u64,
    ) -> Result<Option<CredentialMaterial>, CredentialError>;
    async fn custodied(
        &self,
        sandbox_id: &SandboxId,
    ) -> Result<Vec<CredentialMaterial>, CredentialError>;
    async fn forget(&self, sandbox_id: &SandboxId, generation: u64) -> Result<(), CredentialError>;
    async fn forget_provider(
        &self,
        sandbox_id: &SandboxId,
        provider_key_id: &str,
    ) -> Result<(), CredentialError> {
        for material in self.custodied(sandbox_id).await? {
            if material.authority.provider_key_id == provider_key_id {
                self.forget(sandbox_id, material.authority.generation)
                    .await?;
            }
        }
        Ok(())
    }
    async fn revoke(&self, sandbox: &Sandbox, provider_key_id: &str)
        -> Result<(), CredentialError>;
    async fn settle_revocations(&self, _sandbox: &Sandbox) -> Result<(), CredentialError> {
        Ok(())
    }
}
