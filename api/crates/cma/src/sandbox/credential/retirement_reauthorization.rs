//! Resolve authority from the same owner scope that admitted the managed Sandbox.
use super::*;
use layer2_core::sandbox::{store::WorkAuthority, RetirementAuthorityVersion};

impl ManagedSandboxCredentials {
    pub(crate) fn with_retirement_reauthorization(mut self: Arc<Self>, enabled: bool) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("credential controller is configured before use")
            .retirement_reauthorization_enabled = enabled;
        self
    }

    pub(super) async fn issuance_owner_authority(
        &self,
        sandbox: &Sandbox,
        token: &SecretValue,
    ) -> Option<RetirementAuthorityVersion> {
        if !self.retirement_reauthorization_enabled || !sandbox.has_ordinary_credential_scope() {
            return None;
        }
        match self
            .tokens
            .retirement_authority_version(sandbox, token)
            .await
        {
            Ok(version) => version,
            Err(_) => {
                tracing::debug!(sandbox_id = %sandbox.id, "Owner scope evidence unavailable during credential issuance");
                None
            }
        }
    }

    async fn custody_owner_authority(
        &self,
        sandbox: &Sandbox,
        key_id: &str,
    ) -> Result<Option<RetirementAuthorityVersion>, CredentialError> {
        let Some(intent) = self
            .custody
            .issuance(sandbox)
            .await
            .map_err(custody_error)?
        else {
            return Ok(None);
        };
        let Some(version) = intent.owner_authority else {
            return Ok(None);
        };
        if intent.sandbox_id != sandbox.id || intent.owner != sandbox.owner {
            return Err(CredentialError::Security(
                "credential issuance crossed its sandbox boundary".into(),
            ));
        }
        if intent.state != IssuanceState::Accepted
            || !matches!(intent.identity, IssuanceIdentity::Prepared { provider_identity, .. }
                if provider_identity == self.issuer.provider_identity())
        {
            return Ok(None);
        }
        let Some(material) = self.material(&sandbox.id, intent.generation).await? else {
            return Ok(None);
        };
        let material = owned_material(material, sandbox)?;
        Ok((material.authority.generation == intent.generation
            && material.authority.provider_key_id == key_id)
            .then_some(version))
    }

    async fn retirement_owner_token(
        &self,
        sandbox: &Sandbox,
    ) -> Result<Option<SecretValue>, CredentialError> {
        self.tokens
            .token_for_sandbox(sandbox)
            .await
            .map_err(CredentialError::Unavailable)
    }

    pub(super) async fn retirement_owner_version(
        &self,
        sandbox: &Sandbox,
    ) -> Result<Option<RetirementAuthorityVersion>, CredentialError> {
        if !self.uses_retirement_reauthorization(sandbox) {
            return Ok(None);
        }
        let Some(token) = self.retirement_owner_token(sandbox).await? else {
            return Ok(None);
        };
        self.tokens
            .retirement_authority_version(sandbox, &token)
            .await
    }

    pub(super) async fn revoke_managed_key(
        &self,
        sandbox: &Sandbox,
        id: &str,
    ) -> Result<(), CredentialError> {
        let recovery = sandbox.work.retirement_credential_recovery;
        if !self.uses_retirement_reauthorization(sandbox) {
            if recovery.is_some() {
                return Err(CredentialError::Security(
                    "retirement credential scope changed".into(),
                ));
            }
            let token = self.owner_token(sandbox).await?;
            return self
                .issuer
                .revoke(id, token.expose())
                .await
                .map_err(classify_issuer);
        }
        let token = self.retirement_owner_token(sandbox).await?;
        let issued_under = if token.is_none() || recovery.is_some() {
            self.custody_owner_authority(sandbox, id).await?
        } else {
            None
        };
        let Some(token) = token else {
            return Err(issued_under.map_or_else(
                || CredentialError::Security("the owner has no renewable NyxID authority".into()),
                |previous| CredentialError::OwnerReauthorization {
                    previous: recovery
                        .and_then(|state| state.admitted)
                        .unwrap_or(previous),
                    missing: true,
                },
            ));
        };
        let observed = self
            .tokens
            .retirement_authority_version(sandbox, &token)
            .await;
        let observed = if recovery.is_some() {
            observed?
        } else {
            observed.ok().flatten()
        };
        if let Some(recovery) = recovery {
            let Some(admitted) = recovery.admitted.filter(|admitted| {
                issued_under.is_some_and(|previous| previous.scope == admitted.scope)
            }) else {
                return Err(CredentialError::Security(
                    "retirement credential scope could not be verified".into(),
                ));
            };
            if observed != Some(admitted) {
                return Err(CredentialError::OwnerReauthorization {
                    previous: admitted,
                    missing: false,
                });
            }
        }
        WorkAuthority::current()
            .map_err(|_| CredentialError::Unavailable("retirement work claim expired".into()))?;
        match self.issuer.revoke(id, token.expose()).await {
            Err(KeyIssuerError::Refused(401)) => {
                if let Some(previous) = observed {
                    let issued_under = match issued_under {
                        Some(version) => Some(version),
                        None => self.custody_owner_authority(sandbox, id).await?,
                    };
                    if issued_under.is_some_and(|issued| issued.scope == previous.scope) {
                        return Err(CredentialError::OwnerReauthorization {
                            previous,
                            missing: false,
                        });
                    }
                }
                Err(classify_issuer(KeyIssuerError::Refused(401)))
            }
            result => result.map_err(classify_issuer),
        }
    }
}

pub(in crate::sandbox) fn retain_strongest_refusal(
    failure: &mut Option<CredentialError>,
    error: CredentialError,
) {
    fn priority(error: &CredentialError) -> u8 {
        match error {
            CredentialError::Security(_) | CredentialError::ServiceNotGranted(_) => 2,
            CredentialError::Unavailable(_) | CredentialError::Uncertain => 1,
            CredentialError::OwnerReauthorization { .. } => 0,
        }
    }
    if let (
        Some(CredentialError::OwnerReauthorization {
            previous: current,
            missing: current_missing,
        }),
        CredentialError::OwnerReauthorization {
            previous: observed,
            missing,
        },
    ) = (failure.as_mut(), &error)
    {
        if current.scope != observed.scope {
            *failure = Some(CredentialError::Security(
                "credential revocation requires different owner scopes".into(),
            ));
        } else {
            if observed.granted_at_ms > current.granted_at_ms {
                *current = *observed;
            }
            *current_missing |= *missing;
        }
        return;
    }
    if failure
        .as_ref()
        .is_none_or(|existing| priority(&error) > priority(existing))
    {
        *failure = Some(error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_or_custody_refusals_win_in_either_key_order() {
        let missing = CredentialError::OwnerReauthorization {
            previous: RetirementAuthorityVersion {
                grant: [1; 16],
                scope: [2; 32],
                granted_at_ms: 1,
            },
            missing: true,
        };
        let policy = CredentialError::Security("fixture policy refusal".into());
        for (first, second) in [(missing.clone(), policy.clone()), (policy.clone(), missing)] {
            let mut outcome = None;
            retain_strongest_refusal(&mut outcome, first);
            retain_strongest_refusal(&mut outcome, second);
            assert_eq!(outcome, Some(policy.clone()));
        }
    }
}
