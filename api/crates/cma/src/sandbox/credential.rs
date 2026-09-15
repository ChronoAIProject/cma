use std::sync::Arc;
use std::time::SystemTime;

use layer2_core::sandbox::{Sandbox, SandboxCredentialAuthority};
use layer2_core::{rfc3339_at, SandboxId, ServiceId, SubjectId};

use crate::auth::keys::{KeyIssuer, KeyIssuerError};
use crate::auth::persistence::SecretValue;
use crate::auth::AuthState;

#[derive(Debug, Clone)]
pub(crate) struct CredentialMaterial {
    pub sandbox_id: SandboxId,
    pub owner: SubjectId,
    pub authority: SandboxCredentialAuthority,
    pub value: SecretValue,
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
    /// Check authority before any phase that will restore runtime access.
    /// Ordinary credentials retain their existing lifecycle authorization.
    async fn require_lifecycle_restore(&self, _sandbox: &Sandbox) -> Result<(), CredentialError> {
        Ok(())
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

pub(crate) struct ManagedSandboxCredentials {
    issuer: Arc<KeyIssuer>,
    tokens: Arc<dyn OwnerTokenSource>,
    custody: Arc<dyn SandboxCredentialCustody>,
}

impl ManagedSandboxCredentials {
    pub(crate) fn new<T, C>(issuer: Arc<KeyIssuer>, tokens: Arc<T>, custody: Arc<C>) -> Arc<Self>
    where
        T: OwnerTokenSource + 'static,
        C: SandboxCredentialCustody + 'static,
    {
        Arc::new(Self {
            issuer,
            tokens,
            custody,
        })
    }

    async fn owner_token(&self, owner: &SubjectId) -> Result<SecretValue, CredentialError> {
        self.tokens
            .token_for(owner)
            .await
            .map_err(CredentialError::Unavailable)?
            .ok_or_else(|| {
                CredentialError::Security("the owner has no renewable NyxID authority".into())
            })
    }

    async fn settle_queued(&self, sandbox: &Sandbox) -> Result<(), CredentialError> {
        let revocations = self
            .custody
            .revocations(&sandbox.id)
            .await
            .map_err(custody_error)?;
        if revocations.is_empty() {
            return Ok(());
        }
        if revocations.iter().any(|revocation| {
            revocation.sandbox_id != sandbox.id || revocation.owner != sandbox.owner
        }) {
            return Err(CredentialError::Security(
                "credential revocation crossed its sandbox boundary".into(),
            ));
        }
        let token = self.owner_token(&sandbox.owner).await?;
        for revocation in revocations {
            self.issuer
                .revoke(&revocation.provider_key_id, token.expose())
                .await
                .map_err(classify_issuer)?;
            self.custody
                .delete_revocation(&sandbox.id, &revocation.provider_key_id)
                .await
                .map_err(custody_error)?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl SandboxCredentialController for ManagedSandboxCredentials {
    async fn issue(
        &self,
        sandbox: &Sandbox,
        generation: u64,
        now: SystemTime,
    ) -> Result<CredentialMaterial, CredentialError> {
        if sandbox.validate().is_err()
            || generation <= sandbox.credential.service_access_generation_floor()
        {
            return Err(CredentialError::Security(
                "credential generation does not cover the current service access".into(),
            ));
        }
        self.settle_queued(sandbox).await?;
        if let Some(existing) = self.material(&sandbox.id, generation).await? {
            return owned_material(existing, &sandbox.owner);
        }
        let token = self.owner_token(&sandbox.owner).await?;
        let minted = self
            .issuer
            .mint(
                sandbox.id.as_str(),
                token.expose(),
                &sandbox.credential.effective_reaches(),
            )
            .await
            .map_err(classify_issuer)?;
        let material = CredentialMaterial {
            sandbox_id: sandbox.id.clone(),
            owner: sandbox.owner.clone(),
            authority: SandboxCredentialAuthority {
                generation,
                provider_key_id: minted.id,
                provider_state_version: minted.state_version,
                issued_at: rfc3339_at(now),
                expires_at: rfc3339_at(minted.expires_at),
                grant_fingerprint: minted.grant_fingerprint,
            },
            value: minted.value,
        };
        match self.custody.insert(material.clone()).await {
            Ok(()) => Ok(material),
            Err(CredentialCustodyError::Duplicate) => {
                let winner = self
                    .material(&sandbox.id, generation)
                    .await?
                    .ok_or_else(|| {
                        CredentialError::Unavailable(
                            "credential generation raced but no durable value remained".into(),
                        )
                    })?;
                let winner = owned_material(winner, &sandbox.owner)?;
                if winner.authority.provider_key_id != material.authority.provider_key_id {
                    let revocation = CredentialRevocation {
                        sandbox_id: sandbox.id.clone(),
                        owner: sandbox.owner.clone(),
                        provider_key_id: material.authority.provider_key_id.clone(),
                    };
                    if let Err(error) = self.custody.queue_revocation(revocation).await {
                        let _ = self
                            .issuer
                            .revoke(&material.authority.provider_key_id, token.expose())
                            .await;
                        return Err(custody_error(error));
                    }
                    self.issuer
                        .revoke(&material.authority.provider_key_id, token.expose())
                        .await
                        .map_err(classify_issuer)?;
                    self.custody
                        .delete_revocation(&sandbox.id, &material.authority.provider_key_id)
                        .await
                        .map_err(custody_error)?;
                }
                Ok(winner)
            }
            Err(error) => {
                // A provider key that never reached durable custody must not be
                // left live merely because the custody dependency failed.
                let _ = self
                    .issuer
                    .revoke(&material.authority.provider_key_id, token.expose())
                    .await;
                Err(custody_error(error))
            }
        }
    }

    async fn material(
        &self,
        sandbox_id: &SandboxId,
        generation: u64,
    ) -> Result<Option<CredentialMaterial>, CredentialError> {
        self.custody
            .get(sandbox_id, generation)
            .await
            .map_err(custody_error)
    }

    async fn custodied(
        &self,
        sandbox_id: &SandboxId,
    ) -> Result<Vec<CredentialMaterial>, CredentialError> {
        self.custody.list(sandbox_id).await.map_err(custody_error)
    }

    async fn forget(&self, sandbox_id: &SandboxId, generation: u64) -> Result<(), CredentialError> {
        self.custody
            .delete(sandbox_id, generation)
            .await
            .map_err(custody_error)
    }

    async fn revoke(
        &self,
        sandbox: &Sandbox,
        provider_key_id: &str,
    ) -> Result<(), CredentialError> {
        let token = self.owner_token(&sandbox.owner).await?;
        self.issuer
            .revoke(provider_key_id, token.expose())
            .await
            .map_err(classify_issuer)
    }

    async fn settle_revocations(&self, sandbox: &Sandbox) -> Result<(), CredentialError> {
        self.settle_queued(sandbox).await
    }
}

fn owned_material(
    material: CredentialMaterial,
    owner: &SubjectId,
) -> Result<CredentialMaterial, CredentialError> {
    if &material.owner == owner {
        Ok(material)
    } else {
        Err(CredentialError::Security(
            "credential custody crossed its owner boundary".into(),
        ))
    }
}

fn custody_error(error: CredentialCustodyError) -> CredentialError {
    match error {
        CredentialCustodyError::Unavailable(detail) => CredentialError::Unavailable(detail),
        CredentialCustodyError::Duplicate => {
            CredentialError::Unavailable("credential generation conflict".into())
        }
        CredentialCustodyError::Corrupt(detail) => CredentialError::Security(detail),
    }
}

fn classify_issuer(error: KeyIssuerError) -> CredentialError {
    match error {
        KeyIssuerError::ServiceNotGranted(service) => CredentialError::ServiceNotGranted(service),
        KeyIssuerError::Unavailable(detail) => CredentialError::Unavailable(detail),
        KeyIssuerError::Refused(status) if status == 429 || status >= 500 => {
            CredentialError::Unavailable(format!("identity service returned HTTP {status}"))
        }
        KeyIssuerError::Refused(status) => {
            CredentialError::Security(format!("identity service returned HTTP {status}"))
        }
        other => CredentialError::Security(other.to_string()),
    }
}

#[cfg(test)]
#[path = "credential_tests.rs"]
mod tests;
