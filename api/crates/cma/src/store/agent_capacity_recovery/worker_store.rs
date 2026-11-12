//! Request-scoped production port; every operation reauthorizes the caller.
use super::{service::RecoveryService, worker::AcceptedWork};
use layer2_core::{
    sandbox::agent_capacity_recovery::{
        activation::RecoveryActivation,
        ports::{ClaimOutcome, CommitOutcome, RecoveryError},
        state::{RecoveryClaim, RecoveryConfirmation, RecoveryRequest, RecoveryWork},
    },
    Caller,
};

pub(super) struct AuthorizedWork<'a> {
    pub service: &'a RecoveryService,
    pub caller: &'a Caller,
}

#[async_trait::async_trait]
impl AcceptedWork for AuthorizedWork<'_> {
    async fn claim(
        &self,
        request: &RecoveryRequest,
        holder: &str,
        now: &str,
        expires: &str,
    ) -> Result<ClaimOutcome, RecoveryError> {
        self.service
            .claim(self.caller, request, holder, now, expires)
            .await
    }
    async fn complete(
        &self,
        claim: &RecoveryClaim,
        confirmation: &RecoveryConfirmation,
        now: &str,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        self.service
            .complete(self.caller, claim, confirmation, now)
            .await
    }
    async fn retry(
        &self,
        claim: &RecoveryClaim,
        now: &str,
        retry_at: &str,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        self.service.retry(self.caller, claim, now, retry_at).await
    }
    async fn reconcile(
        &self,
        expected: &RecoveryWork,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        self.service.reconcile(self.caller, expected).await
    }
    async fn activation(
        &self,
        completed: &RecoveryWork,
    ) -> Result<RecoveryActivation, RecoveryError> {
        self.service.activation(self.caller, completed).await
    }
    async fn require_activation(&self, completed: &RecoveryWork) -> Result<(), RecoveryError> {
        self.service
            .require_activation(self.caller, completed)
            .await
    }
}
