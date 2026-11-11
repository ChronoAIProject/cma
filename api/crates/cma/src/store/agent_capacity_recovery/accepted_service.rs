//! Accepted background work freshly authorized by its exact publication.
use super::{
    accepted_authority, settlement, transactions,
    worker::{self, AcceptedWork},
    RecoveryService,
};
use layer2_core::sandbox::agent_capacity_recovery::{
    activation::RecoveryActivation,
    ports::{CapacityRecoveryRuntime, ClaimOutcome, CommitOutcome, RecoveryError},
    state::{RecoveryClaim, RecoveryConfirmation, RecoveryRequest, RecoveryWork},
};

impl RecoveryService {
    /// Discovery supplies only the original request. Every store operation
    /// independently proves current personal ownership and the exact result;
    /// no Caller, admission, ordinary reconnect or input dispatch is available.
    #[allow(
        dead_code,
        reason = "accepted recovery service remains closed until qualification"
    )]
    pub(crate) async fn run_accepted<R: CapacityRecoveryRuntime>(
        &self,
        runtime: &R,
        request: &RecoveryRequest,
        holder: &str,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        worker::run(
            &DurableAcceptedWork(self),
            runtime,
            request,
            holder,
            std::time::SystemTime::now(),
        )
        .await
    }
}

#[allow(
    dead_code,
    reason = "accepted recovery service remains closed until qualification"
)]
struct DurableAcceptedWork<'a>(&'a RecoveryService);

#[async_trait::async_trait]
impl AcceptedWork for DurableAcceptedWork<'_> {
    async fn claim(
        &self,
        request: &RecoveryRequest,
        holder: &str,
        now: &str,
        expires: &str,
    ) -> Result<ClaimOutcome, RecoveryError> {
        let authorized = accepted_authority::authorize(&self.0.store, request).await?;
        transactions::claim(
            &self.0.store,
            settlement::ClaimRequest {
                authorized: &authorized,
                request,
                holder,
                now,
                expires_at: expires,
            },
        )
        .await
    }
    async fn complete(
        &self,
        claim: &RecoveryClaim,
        confirmation: &RecoveryConfirmation,
        now: &str,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        let authorized = accepted_authority::authorize(&self.0.store, &claim.work.request).await?;
        transactions::complete(&self.0.store, &authorized, claim, confirmation, now).await
    }
    async fn retry(
        &self,
        claim: &RecoveryClaim,
        now: &str,
        retry_at: &str,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        let authorized = accepted_authority::authorize(&self.0.store, &claim.work.request).await?;
        transactions::retry(&self.0.store, &authorized, claim, now, retry_at).await
    }
    async fn reconcile(
        &self,
        expected: &RecoveryWork,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        let authorized = accepted_authority::authorize(&self.0.store, &expected.request).await?;
        transactions::reconcile(&self.0.store, &authorized, expected).await
    }
    async fn activation(
        &self,
        completed: &RecoveryWork,
    ) -> Result<RecoveryActivation, RecoveryError> {
        let authorized = accepted_authority::authorize(&self.0.store, &completed.request).await?;
        self.0.activation_authorized(&authorized, completed).await
    }
    async fn require_activation(&self, completed: &RecoveryWork) -> Result<(), RecoveryError> {
        self.activation(completed).await.map(|_| ())
    }
}
