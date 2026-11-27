use crate::auth::{persistence::SecretValue, AuthState};
use layer2_core::{
    sandbox::{Sandbox, SandboxCredentialAuthority},
    SandboxId, ServiceId, SubjectId,
};
use layer3_adapter::{CredentialFrame, RuntimeCredential};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub(crate) struct CredentialMaterial {
    pub source: layer2_core::sandbox::SandboxCredentialSource,
    pub sandbox_id: SandboxId,
    pub owner: SubjectId,
    pub authority: SandboxCredentialAuthority,
    pub value: SecretValue,
    pub github_route: Option<layer3_adapter::GithubCredentialRoute>,
}

impl CredentialMaterial {
    pub(crate) fn into_runtime_frame(self, operation_id: String) -> CredentialFrame {
        CredentialFrame {
            operation_id,
            value: RuntimeCredential::new(self.value.expose()),
            custody_generation: Some(self.authority.generation),
            github_route: self.github_route,
        }
    }

    pub(crate) fn matches_connection(&self, sandbox: &Sandbox) -> bool {
        let expected = sandbox.credential.selected_connection();
        match (expected, &self.github_route) {
            (None, None) => true,
            (Some(selection), Some(route)) => {
                selection.service.as_str() == crate::trigger::GITHUB_SERVICE
                    && selection.connection_id == route.service_id
                    && route.is_valid()
                    && match (
                        sandbox.credential.selected_repository_connection(),
                        &route.repository_scope,
                    ) {
                        (None, None) => true,
                        (Some(initial), Some(scope)) => {
                            use layer2_core::sandbox::RepositoryCredentialRequirement as Need;
                            scope.repository == initial.repository
                                && matches!(
                                    (initial.requirement, scope.required_scope),
                                    (
                                        Need::PublicRepositoryWrite,
                                        layer3_adapter::GithubRepositoryScopeNeed::PublicRepo
                                    ) | (
                                        Need::PrivateRepositoryRead,
                                        layer3_adapter::GithubRepositoryScopeNeed::Repo
                                    )
                                )
                        }
                        _ => false,
                    }
            }
            _ => false,
        }
    }

    pub(crate) fn matches_source(&self) -> bool {
        use layer2_core::sandbox::SandboxCredentialSource;
        use sha2::{Digest, Sha256};
        self.source.accepts(&self.authority)
            && self
                .github_route
                .as_ref()
                .is_none_or(|route| self.source.is_managed() && route.is_valid())
            && match &self.source {
                SandboxCredentialSource::Managed
                | SandboxCredentialSource::ManagedConnection { .. } => true,
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
    async fn requires_no_resend(&self, _sandbox: &Sandbox) -> Result<bool, CredentialCustodyError> {
        Ok(false)
    }

    async fn issuance(
        &self,
        _sandbox: &Sandbox,
    ) -> Result<Option<super::CredentialIssuance>, CredentialCustodyError> {
        Err(CredentialCustodyError::Unavailable(
            "issuance custody is not supported".into(),
        ))
    }
    async fn begin_issuance(
        &self,
        _sandbox: &Sandbox,
        _intent: &super::CredentialIssuance,
    ) -> Result<bool, CredentialCustodyError> {
        Err(CredentialCustodyError::Unavailable(
            "issuance custody is not supported".into(),
        ))
    }
    async fn accept_issuance(
        &self,
        _sandbox: &Sandbox,
        _intent: &super::CredentialIssuance,
        _material: &CredentialMaterial,
    ) -> Result<(), CredentialCustodyError> {
        Err(CredentialCustodyError::Unavailable(
            "issuance custody is not supported".into(),
        ))
    }
    async fn has_retirement_receipt(
        &self,
        _sandbox: &Sandbox,
        _intent: &super::CredentialIssuance,
    ) -> Result<bool, CredentialCustodyError> {
        Ok(false)
    }
    async fn record_retirement_receipt(
        &self,
        _sandbox: &Sandbox,
        _intent: &super::CredentialIssuance,
        _receipt: &crate::auth::keys::CredentialRetirementReceipt,
    ) -> Result<(), CredentialCustodyError> {
        Err(CredentialCustodyError::Unavailable(
            "credential retirement receipts are not supported".into(),
        ))
    }
    async fn observe_issuance(
        &self,
        _sandbox: &Sandbox,
        _intent: &super::CredentialIssuance,
        _observation: super::IssuanceObservation,
        _now: SystemTime,
    ) -> Result<(), CredentialCustodyError> {
        Err(CredentialCustodyError::Unavailable(
            "issuance custody is not supported".into(),
        ))
    }

    /// Retain supplied material only. Managed material requires an issuance intent
    /// and current work authority through `accept_issuance`.
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
    #[cfg(test)]
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
    #[error("verified owner reauthorization is required")]
    OwnerReauthorization {
        previous: layer2_core::sandbox::RetirementAuthorityVersion,
        missing: bool,
    },
    #[error("credential issuance is uncertain; retry reconciliation")]
    Uncertain,
    #[error("service is not granted: {0}")]
    ServiceNotGranted(ServiceId),
    #[error("credential dependency unavailable: {0}")]
    Unavailable(String),
    #[error("credential authority was refused: {0}")]
    Security(String),
}

#[async_trait::async_trait]
pub(crate) trait OwnerTokenSource: Send + Sync {
    async fn retirement_authority_version(
        &self,
        _sandbox: &Sandbox,
        _token: &SecretValue,
    ) -> Result<Option<layer2_core::sandbox::RetirementAuthorityVersion>, CredentialError> {
        Ok(None)
    }
    async fn token_for(&self, owner: &SubjectId) -> Result<Option<SecretValue>, String>;
    async fn token_for_sandbox(&self, sandbox: &Sandbox) -> Result<Option<SecretValue>, String> {
        self.token_for(&sandbox.owner).await
    }
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
    fn uses_retirement_reauthorization(&self, _sandbox: &Sandbox) -> bool {
        false
    }

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
    async fn retirement_authority_version(
        &self,
        _sandbox: &Sandbox,
    ) -> Result<Option<layer2_core::sandbox::RetirementAuthorityVersion>, CredentialError> {
        Ok(None)
    }
    async fn settle_revocations(&self, _sandbox: &Sandbox) -> Result<(), CredentialError> {
        Ok(())
    }
}
