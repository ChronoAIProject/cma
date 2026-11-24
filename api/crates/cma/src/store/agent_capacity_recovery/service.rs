//! Production accepted-work entry. Every call obtains fresh resource authority;
//! an admission switch never cancels receipt lookup or accepted settlement.
use crate::store::transaction_terminal::GuardedTerminal as _;
use std::sync::Arc;

use layer2_core::{
    resource_access::{
        AuthorizedResource, ResourceAccessService, ResourceCapability, ResourceTarget,
    },
    sandbox::agent_capacity_recovery::{
        activation::RecoveryActivation,
        ports::{ClaimOutcome, CommitOutcome, RecoveryError, RecoveryLookup},
        service::CapacityRecoveryFeatures,
        state::{RecoveryClaim, RecoveryConfirmation, RecoveryRequest, RecoveryWork},
    },
    AgentId, Caller,
};

use super::{
    admission::AdmissionRequest,
    discovery,
    observations::{cohort, Outcome, Stage},
    preparation, settlement, status, transactions, RecoveryCursor, RecoveryPage, RecoveryStatus,
    RecoveryStream,
};
use crate::store::{
    agent_log_segments::transaction_scope::TransactionScope, sandbox_mongo::MongoSandboxStore,
};

pub(crate) struct RecoveryService {
    pub(super) store: MongoSandboxStore,
    access: Arc<ResourceAccessService>,
    features: CapacityRecoveryFeatures,
}

impl RecoveryService {
    pub(super) async fn require_activation(
        &self,
        caller: &Caller,
        completed: &RecoveryWork,
    ) -> Result<(), RecoveryError> {
        self.activation(caller, completed).await.map(|_| ())
    }

    pub(super) async fn activation(
        &self,
        caller: &Caller,
        completed: &RecoveryWork,
    ) -> Result<RecoveryActivation, RecoveryError> {
        let authorized = self.authorize(caller, &completed.request.agent_id).await?;
        self.activation_authorized(&authorized, completed).await
    }

    pub(super) async fn activation_authorized(
        &self,
        authorized: &dyn super::authority::RecoveryAuthority,
        completed: &RecoveryWork,
    ) -> Result<RecoveryActivation, RecoveryError> {
        authorized
            .require_request(&completed.request)
            .map_err(transactions::error)?;
        let mut session = self
            .store
            .client
            .start_session()
            .await
            .map_err(|_| RecoveryError::Unavailable)?;
        let mut scope = TransactionScope::start(&mut session)
            .await
            .map_err(|_| RecoveryError::Unavailable)?;
        let checked = async {
            let current = super::authority::capture(
                &self.store,
                authorized,
                &completed.request.agent_id,
                &mut scope,
            )
            .await?;
            let context = current.context()?;
            let stored =
                super::receipts::read(&self.store, context.agent, &completed.request, &mut scope)
                    .await?
                    .ok_or(layer2_core::sandbox::store::SandboxStoreError::Conflict)?;
            if stored.work != *completed {
                return Err(layer2_core::sandbox::store::SandboxStoreError::Conflict.into());
            }
            super::receipts::require_current_result(
                &self.store,
                context.agent,
                &stored,
                &mut scope,
            )
            .await?;
            let activation = RecoveryActivation::capture_current(&context, completed)
                .map_err(|_| layer2_core::sandbox::store::SandboxStoreError::Conflict)?;
            super::authority::require(authorized)?;
            Ok(activation)
        }
        .await;
        let _ = scope.release().abort_guarded().await;
        checked.map_err(transactions::error)
    }
    pub(crate) fn new(store: MongoSandboxStore, access: Arc<ResourceAccessService>) -> Self {
        Self {
            store,
            access,
            features: CapacityRecoveryFeatures::default(),
        }
    }

    pub(crate) fn with_features(mut self, features: CapacityRecoveryFeatures) -> Self {
        self.features = features;
        self
    }

    async fn authorize(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<AuthorizedResource, RecoveryError> {
        self.access
            .authorize_personal(
                caller,
                &ResourceTarget::Agent {
                    agent_id: id.clone(),
                },
                ResourceCapability::OperateAgent,
            )
            .await
            .map_err(|_| RecoveryError::Unauthorized)
    }

    pub(crate) async fn status(
        &self,
        caller: &Caller,
        id: &AgentId,
        request: Option<&RecoveryRequest>,
    ) -> Result<RecoveryStatus, RecoveryError> {
        let authorized = self.authorize(caller, id).await?;
        let mut session = self
            .store
            .client
            .start_session()
            .await
            .map_err(|_| RecoveryError::Unavailable)?;
        let mut scope = TransactionScope::start(&mut session)
            .await
            .map_err(|_| RecoveryError::Unavailable)?;
        let result = status::read(
            &self.store,
            &authorized,
            id,
            request,
            &layer2_core::now_rfc3339(),
            self.features.enabled || self.store.segment_adoption_enabled,
            &mut scope,
        )
        .await;
        let _ = scope.release().abort_guarded().await;
        result.map_err(transactions::error)
    }

    pub(crate) async fn lookup(
        &self,
        caller: &Caller,
        request: &RecoveryRequest,
    ) -> Result<RecoveryLookup, RecoveryError> {
        let authorized = self.authorize(caller, &request.agent_id).await?;
        transactions::lookup_accepted(&self.store, &authorized, request).await
    }

    pub(crate) async fn admit(
        &self,
        caller: &Caller,
        request: &RecoveryRequest,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        request.validate()?;
        if let RecoveryLookup::Recorded(work) =
            self.lookup(caller, request).await.inspect_err(|_error| {
                #[cfg(test)]
                eprintln!("capacity recovery original lookup refused: {_error:?}");
            })?
        {
            cohort(Stage::Admitted, Outcome::Replay);
            return Ok(CommitOutcome::Recorded(*work));
        }
        if !self.features.enabled {
            return Err(RecoveryError::Disabled);
        }
        let authorized = self.authorize(caller, &request.agent_id).await?;
        let prepared = match preparation::prepare(&self.store, &authorized, request)
            .await
            .inspect_err(|_error| {
                #[cfg(test)]
                eprintln!("capacity recovery preparation refused: {_error:?}");
            })? {
            CommitOutcome::Recorded(prepared) => prepared,
            CommitOutcome::KnownAbort => return Ok(CommitOutcome::KnownAbort),
            CommitOutcome::Unknown => return Ok(CommitOutcome::Unknown),
        };
        // Bootstrap may take time. Obtain current authority again; the final
        // transaction recaptures source, parents and every original input fact.
        let authorized = self.authorize(caller, &request.agent_id).await?;
        let now = layer2_core::now_rfc3339();
        let result = transactions::admit(
            &self.store,
            AdmissionRequest {
                authorized: &authorized,
                request,
                admission: &prepared.admission,
                bootstrap: prepared.bootstrap.as_ref(),
                now: &now,
            },
            self.features,
        )
        .await
        .inspect_err(|_error| {
            #[cfg(test)]
            eprintln!("capacity recovery admission transaction refused: {_error:?}");
        })?;
        Ok(match result {
            CommitOutcome::Recorded(value) => {
                // The committed receipt must bind the exact proposal. Never
                // activate from a successfully inserted carrier alone.
                if value.agent.id != value.work.request.agent_id
                    || value.publication.publication_key != request.key()?
                {
                    return Err(RecoveryError::InvalidEvidence);
                }
                CommitOutcome::Recorded(value.work)
            }
            CommitOutcome::KnownAbort => CommitOutcome::KnownAbort,
            CommitOutcome::Unknown => CommitOutcome::Unknown,
        })
    }

    pub(crate) async fn claim(
        &self,
        caller: &Caller,
        request: &RecoveryRequest,
        holder: &str,
        now: &str,
        expires_at: &str,
    ) -> Result<ClaimOutcome, RecoveryError> {
        let authorized = self.authorize(caller, &request.agent_id).await?;
        transactions::claim(
            &self.store,
            settlement::ClaimRequest {
                authorized: &authorized,
                request,
                holder,
                now,
                expires_at,
            },
        )
        .await
    }

    pub(crate) async fn complete(
        &self,
        caller: &Caller,
        claim: &RecoveryClaim,
        confirmation: &RecoveryConfirmation,
        now: &str,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        let authorized = self.authorize(caller, &claim.work.request.agent_id).await?;
        transactions::complete(&self.store, &authorized, claim, confirmation, now).await
    }

    pub(crate) async fn retry(
        &self,
        caller: &Caller,
        claim: &RecoveryClaim,
        now: &str,
        retry_at: &str,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        let authorized = self.authorize(caller, &claim.work.request.agent_id).await?;
        transactions::retry(&self.store, &authorized, claim, now, retry_at).await
    }

    pub(crate) async fn reconcile(
        &self,
        caller: &Caller,
        expected: &RecoveryWork,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        let authorized = self.authorize(caller, &expected.request.agent_id).await?;
        transactions::reconcile(&self.store, &authorized, expected).await
    }

    /// Expose bounded per-Agent lookup to Mongo contract tests. Production
    /// restart consumes discovery::for_agent directly within its receipt check.
    #[cfg(test)]
    pub(crate) async fn discover_agent(
        &self,
        agent_id: &AgentId,
    ) -> Result<Vec<RecoveryRequest>, RecoveryError> {
        discovery::for_agent(&self.store, agent_id).await
    }

    pub(crate) async fn discover(
        &self,
        stream: RecoveryStream,
        after: Option<&RecoveryCursor>,
        now: &str,
    ) -> Result<RecoveryPage, RecoveryError> {
        discovery::page(&self.store, stream, after, now).await
    }
}
