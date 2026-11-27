use super::*;
use layer2_core::sandbox::store::{ClaimedWork, WorkAuthority};
use layer2_core::sandbox::SandboxStatus;

impl ManagedSandboxCredentials {
    pub(crate) fn with_retirement_receipts(mut self: Arc<Self>, enabled: bool) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("credential controller is configured before use")
            .retirement_receipts_enabled = enabled;
        self
    }

    pub(super) async fn settle_unknown_retirement(
        &self,
        sandbox: &Sandbox,
        intent: &CredentialIssuance,
    ) -> Result<(), CredentialError> {
        if sandbox.status != SandboxStatus::Retiring || !sandbox.work.step.is_retirement() {
            return Err(CredentialError::Uncertain);
        }
        if self
            .custody
            .has_retirement_receipt(sandbox, intent)
            .await
            .map_err(custody_error)?
        {
            return Ok(());
        }
        let IssuanceIdentity::Prepared {
            provider_identity,
            request_digest,
        } = &intent.identity
        else {
            return Err(CredentialError::Uncertain);
        };
        if !self.retirement_receipts_enabled
            || *provider_identity != self.issuer.provider_identity()
        {
            return Err(CredentialError::Uncertain);
        }
        // The read cannot mint or revoke. The store checks the live retirement
        // lease again when it accepts the provider's disposition.
        require_retirement_claim(sandbox)?;
        let token = self.owner_token(sandbox).await?;
        let receipt = self
            .issuer
            .retirement_receipt(
                &intent.operation_id,
                &sandbox.owner,
                request_digest,
                token.expose(),
            )
            .await
            .ok()
            .flatten()
            .ok_or(CredentialError::Uncertain)?;
        require_retirement_claim(sandbox)?;
        self.custody
            .record_retirement_receipt(sandbox, intent, &receipt)
            .await
            .map_err(custody_error)
    }
}

fn require_retirement_claim(sandbox: &Sandbox) -> Result<(), CredentialError> {
    let lease = sandbox
        .work
        .lease
        .as_ref()
        .ok_or(CredentialError::Uncertain)?;
    let current = WorkAuthority::current().map_err(|_| CredentialError::Uncertain)?;
    current
        .iter()
        .any(|authority| {
            matches!(authority.claimed(), ClaimedWork::Sandbox(row)
            if row.id == sandbox.id && row.owner == sandbox.owner
                && row.status == SandboxStatus::Retiring && row.work.step == sandbox.work.step
                && row.work.lease.as_ref().is_some_and(|held|
                    held.holder == lease.holder && held.fence == lease.fence))
        })
        .then_some(())
        .ok_or(CredentialError::Uncertain)
}
