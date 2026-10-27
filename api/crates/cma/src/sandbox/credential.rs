use std::sync::Arc;
use std::time::SystemTime;

use layer2_core::sandbox::{Sandbox, SandboxCredentialAuthority};
use layer2_core::{rfc3339_at, SandboxId, SubjectId};

use crate::auth::keys::{KeyIssuer, KeyIssuerError};
use crate::auth::persistence::SecretValue;

mod contracts;
mod provided;
pub(crate) use contracts::*;

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
        if !sandbox.credential.source.is_managed() {
            for revocation in revocations {
                self.custody
                    .delete_revocation(&sandbox.id, &revocation.provider_key_id)
                    .await
                    .map_err(custody_error)?;
            }
            return Ok(());
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
            return owned_material(existing, sandbox);
        }
        if !sandbox.credential.source.is_managed() {
            return Err(CredentialError::Security(
                "provided credential is absent from custody".into(),
            ));
        }
        let token = self.owner_token(&sandbox.owner).await?;
        let minted = self
            .issuer
            .mint(
                sandbox.id.as_str(),
                &sandbox.owner,
                token.expose(),
                &sandbox.credential.effective_reaches(),
            )
            .await
            .map_err(classify_issuer)?;
        let material = CredentialMaterial {
            source: Default::default(),
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
                let winner = owned_material(winner, sandbox)?;
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
        if !sandbox.credential.source.is_managed() {
            return Ok(());
        }
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
    sandbox: &Sandbox,
) -> Result<CredentialMaterial, CredentialError> {
    if material.owner == sandbox.owner
        && material.sandbox_id == sandbox.id
        && material.source == sandbox.credential.source
        && material.matches_source()
    {
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
