//! Default-off E1 persistence, with no provider/runtime mutation capability.

use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionRefusal as Refusal;
use layer2_core::sandbox::ledger_maintenance_pre_exclusion::LedgerPreExclusionStateV1 as State;
use layer2_core::sandbox::ledger_maintenance_pre_exclusion_progress::{
    LedgerPreExclusionAdmission, LedgerPreExclusionPolicy as Policy, LedgerPreExclusionProgress,
};
use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStoreError as Domain;
use layer2_core::sandbox::store::ledger_maintenance_pre_exclusion::{
    LedgerPreExclusionInspection, LedgerPreExclusionStore,
};
use layer2_core::{Caller, SandboxId};

use super::ledger_maintenance::ApplyError;
use super::ledger_maintenance_pre_exclusion_transaction as transaction;
use super::ledger_maintenance_snapshot;
use super::sandbox_mongo::{
    commit_transaction, is_transient, start_transaction, wait_for_reconciliation, CommitOutcome,
    MongoSandboxStore, TRANSACTION_ATTEMPTS,
};

pub(super) enum Change {
    Admit(LedgerPreExclusionAdmission),
    Progress(LedgerPreExclusionProgress),
}

impl Change {
    pub(super) fn caller(&self) -> &Caller {
        match self {
            Self::Admit(r) => &r.caller,
            Self::Progress(r) => &r.caller,
        }
    }
    pub(super) fn sandbox(&self) -> &SandboxId {
        match self {
            Self::Admit(r) => &r.accepted.plan().sandbox_id,
            Self::Progress(r) => &r.expected_source.sandbox.id,
        }
    }
}

#[async_trait::async_trait]
impl LedgerPreExclusionStore for MongoSandboxStore {
    async fn inspect_pre_exclusion(
        &self,
        caller: &Caller,
        sandbox: &SandboxId,
    ) -> Result<LedgerPreExclusionInspection, Domain> {
        let mut session = self
            .client
            .start_session()
            .await
            .map_err(|_| Domain::Unavailable)?;
        start_transaction(&mut session)
            .await
            .map_err(|_| Domain::Unavailable)?;
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            ledger_maintenance_snapshot::load_pre_exclusion(self, caller, sandbox, &mut session),
        )
        .await
        .map_err(|_| Domain::Unavailable)
        .and_then(|result| result.map_err(ApplyError::domain));
        session
            .abort_transaction()
            .await
            .map_err(|_| Domain::Unavailable)?;
        result.and_then(|snapshot| {
            if snapshot.inspection.state.is_some() {
                return Err(Refusal::Conflict.into());
            }
            Ok(LedgerPreExclusionInspection {
                source: snapshot.inspection.source,
                state: snapshot.pre_exclusion,
            })
        })
    }

    async fn admit_pre_exclusion(
        &self,
        policy: Policy,
        request: LedgerPreExclusionAdmission,
    ) -> Result<State, Domain> {
        execute(self, policy, Change::Admit(request)).await
    }

    async fn progress_pre_exclusion(
        &self,
        policy: Policy,
        request: LedgerPreExclusionProgress,
    ) -> Result<State, Domain> {
        execute(self, policy, Change::Progress(request)).await
    }
}

async fn execute(
    store: &MongoSandboxStore,
    policy: Policy,
    change: Change,
) -> Result<State, Domain> {
    if !policy.enabled {
        return Err(Refusal::Disabled.into());
    }
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = store
            .client
            .start_session()
            .await
            .map_err(|_| Domain::Unavailable)?;
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) {
                continue;
            }
            return Err(Domain::Unavailable);
        }
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            transaction::apply(store, &policy, &change, &mut session),
        )
        .await;
        let receipt = match result.unwrap_or(Err(ApplyError::Domain(Domain::Unavailable))) {
            Ok(receipt) => receipt,
            Err(ApplyError::Driver(error)) if is_transient(&error) => {
                let _ = session.abort_transaction().await;
                continue;
            }
            Err(error) => {
                let _ = session.abort_transaction().await;
                return Err(error.domain());
            }
        };
        let Some(audit) = receipt.audit else {
            session
                .abort_transaction()
                .await
                .map_err(|_| Domain::Unavailable)?;
            return Ok(receipt.state);
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(receipt.state),
            CommitOutcome::Retry(_) => {}
            CommitOutcome::Unknown(_) => {
                let time = session.operation_time().ok_or(Domain::PendingCommit)?;
                let store = store.clone();
                return wait_for_reconciliation("ledger pre-exclusion commit", async move {
                    if transaction::reconcile(&store, &audit, time).await? {
                        Ok(receipt.state)
                    } else {
                        Err(Domain::PendingCommit)
                    }
                })
                .await
                .map_err(|_| Domain::PendingCommit)?;
            }
            CommitOutcome::Failed(_) => return Err(Domain::Unavailable),
        }
        tokio::task::yield_now().await;
    }
    Err(Domain::Unavailable)
}

#[cfg(test)]
#[path = "ledger_maintenance_pre_exclusion_tests.rs"]
mod tests;
