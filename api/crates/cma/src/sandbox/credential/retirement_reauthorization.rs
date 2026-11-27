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

    pub(super) fn uses_retirement_reauthorization(&self, sandbox: &Sandbox) -> bool {
        self.retirement_reauthorization_enabled && sandbox.allows_retirement_credential_recovery()
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
        let token = self.retirement_owner_token(sandbox).await?.ok_or(
            CredentialError::OwnerReauthorization(recovery.and_then(|state| state.admitted)),
        )?;
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
            if observed != recovery.admitted || observed.is_none() {
                return Err(CredentialError::OwnerReauthorization(recovery.admitted));
            }
        }
        WorkAuthority::current()
            .map_err(|_| CredentialError::Unavailable("retirement work claim expired".into()))?;
        match self.issuer.revoke(id, token.expose()).await {
            Err(KeyIssuerError::Refused(401)) => observed.map_or_else(
                || Err(classify_issuer(KeyIssuerError::Refused(401))),
                |version| Err(CredentialError::OwnerReauthorization(Some(version))),
            ),
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
            CredentialError::OwnerReauthorization(_) => 0,
        }
    }
    let newer_rejection = matches!((failure.as_ref(), &error),
        (Some(CredentialError::OwnerReauthorization(current)), CredentialError::OwnerReauthorization(Some(observed)))
            if current.is_none_or(|version| observed.granted_at_ms > version.granted_at_ms));
    if newer_rejection
        || failure
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
        let missing = CredentialError::OwnerReauthorization(None);
        let policy = CredentialError::Security("fixture policy refusal".into());
        for (first, second) in [(missing.clone(), policy.clone()), (policy.clone(), missing)] {
            let mut outcome = None;
            retain_strongest_refusal(&mut outcome, first);
            retain_strongest_refusal(&mut outcome, second);
            assert_eq!(outcome, Some(policy.clone()));
        }
    }
}
