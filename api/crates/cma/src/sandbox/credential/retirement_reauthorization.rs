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
        if !self.retirement_reauthorization_enabled || !sandbox.credential.source.is_managed() {
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
        let intent = self
            .custody
            .issuance(sandbox)
            .await
            .map_err(custody_error)?;
        if let Some(intent) = intent {
            if intent.sandbox_id != sandbox.id || intent.owner != sandbox.owner {
                return Err(CredentialError::Security(
                    "credential issuance crossed its sandbox boundary".into(),
                ));
            }
            if matches!(&intent.identity, IssuanceIdentity::Prepared { provider_identity, .. }
                if *provider_identity != self.issuer.provider_identity())
            {
                return Err(CredentialError::Security(
                    "credential issuance belongs to a different provider".into(),
                ));
            }
            if intent.state == IssuanceState::Accepted
                && matches!(intent.identity, IssuanceIdentity::Prepared { .. })
            {
                if let Some(material) = self.material(&sandbox.id, intent.generation).await? {
                    Self::check_retirement_material(sandbox, &material)?;
                    if material.authority.generation == intent.generation
                        && material.authority.provider_key_id == key_id
                    {
                        return Ok(intent.owner_authority);
                    }
                }
            }
        }
        Ok(None)
    }

    fn check_retirement_material(
        sandbox: &Sandbox,
        material: &CredentialMaterial,
    ) -> Result<(), CredentialError> {
        if material.sandbox_id != sandbox.id
            || material.owner != sandbox.owner
            || material.source != sandbox.credential.source
            || !material.matches_source()
        {
            return Err(CredentialError::Security(
                "credential custody crossed its sandbox boundary".into(),
            ));
        }
        Ok(())
    }

    async fn proves_retirement_key(
        &self,
        sandbox: &Sandbox,
        id: &str,
    ) -> Result<bool, CredentialError> {
        if sandbox
            .credential
            .current
            .iter()
            .chain(&sandbox.credential.pending)
            .any(|key| key.provider_key_id == id)
            || sandbox
                .credential
                .revoke_debt
                .iter()
                .any(|debt| debt.provider_key_id == id)
        {
            return Ok(true);
        }
        for material in self
            .custody
            .list(&sandbox.id)
            .await
            .map_err(custody_error)?
        {
            Self::check_retirement_material(sandbox, &material)?;
            if material.authority.provider_key_id == id {
                return Ok(true);
            }
        }
        for debt in self
            .custody
            .revocations(&sandbox.id)
            .await
            .map_err(custody_error)?
        {
            if debt.owner != sandbox.owner || debt.sandbox_id != sandbox.id {
                return Err(CredentialError::Security(
                    "credential revocation crossed its sandbox boundary".into(),
                ));
            }
            if debt.provider_key_id == id {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn retirement_key_baseline(
        &self,
        sandbox: &Sandbox,
        id: &str,
    ) -> Result<Option<RetirementAuthorityVersion>, CredentialError> {
        let historical = self.custody_owner_authority(sandbox, id).await?;
        if historical.is_none() && !self.proves_retirement_key(sandbox, id).await? {
            return Ok(None);
        }
        match self.tokens.retirement_baseline(sandbox).await? {
            Some(mut baseline) => {
                if let Some(previous) =
                    historical.filter(|previous| previous.scope == baseline.scope)
                {
                    baseline.granted_at_ms = previous.granted_at_ms;
                }
                Ok(Some(baseline))
            }
            None => Ok(historical),
        }
    }

    async fn retirement_owner_token(
        &self,
        sandbox: &Sandbox,
    ) -> Result<Option<SecretValue>, CredentialError> {
        self.tokens
            .retirement_token(sandbox)
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
        let original = if recovery.is_none() {
            self.tokens.token_for_sandbox(sandbox).await.ok().flatten()
        } else {
            None
        };
        let uses_original = original.is_some();
        let token = match original {
            Some(token) => Some(token),
            None => self.retirement_owner_token(sandbox).await?,
        };
        let issued_under = if !uses_original {
            self.retirement_key_baseline(sandbox, id).await?
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
        let observed = if uses_original {
            observed.ok().flatten()
        } else {
            observed?
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
        } else if !uses_original {
            let Some(previous) = issued_under else {
                return Err(CredentialError::Security(
                    "retirement key custody could not be verified".into(),
                ));
            };
            if observed.is_none_or(|proof| proof.scope != previous.scope) {
                return Err(CredentialError::OwnerReauthorization {
                    previous,
                    missing: true,
                });
            }
        }
        WorkAuthority::current()
            .map_err(|_| CredentialError::Unavailable("retirement work claim expired".into()))?;
        match self.issuer.revoke(id, token.expose()).await {
            Err(KeyIssuerError::Refused(401)) => {
                let baseline = match issued_under {
                    Some(version) => Some(version),
                    None => self.retirement_key_baseline(sandbox, id).await?,
                };
                if let Some(baseline) = baseline {
                    if let Some(previous) = observed
                        .filter(|proof| proof.scope == baseline.scope)
                        .or_else(|| baseline.token_generation.map(|_| baseline))
                    {
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
                token_generation: None,
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
