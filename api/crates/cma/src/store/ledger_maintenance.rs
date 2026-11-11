//! Default-off durable admission. No native work, release or lifecycle mutation.

use layer2_core::sandbox::ledger_maintenance_admission::{
    LedgerMaintenanceAdmissionPolicy as Policy, LedgerMaintenanceAdmissionRefusal as Refusal,
    LedgerMaintenanceStateV1,
};
use layer2_core::sandbox::ledger_maintenance_progress::LedgerMaintenanceProgress;
use layer2_core::sandbox::store::ledger_maintenance::{
    LedgerMaintenanceAdmission, LedgerMaintenanceInspection, LedgerMaintenanceProgressStore,
    LedgerMaintenanceStore, LedgerMaintenanceStoreError as Domain, LedgerMaintenanceTakeover,
};
use layer2_core::{Caller, SandboxId};

use super::ledger_maintenance_snapshot;
use super::ledger_maintenance_transaction as transaction;
use super::sandbox_mongo::{
    commit_transaction, is_transient, start_transaction, wait_for_reconciliation, CommitOutcome,
    MongoSandboxStore, TRANSACTION_ATTEMPTS,
};

pub(super) enum ApplyError {
    Domain(Domain),
    Driver(mongodb::error::Error),
}

impl From<Domain> for ApplyError {
    fn from(error: Domain) -> Self {
        Self::Domain(error)
    }
}
impl From<mongodb::error::Error> for ApplyError {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Driver(error)
    }
}
impl ApplyError {
    pub(super) fn domain(self) -> Domain {
        match self {
            Self::Domain(error) => error,
            Self::Driver(_) => Domain::Unavailable,
        }
    }
}

pub(super) fn refused(reason: Refusal) -> ApplyError {
    Domain::Refused(reason).into()
}

pub(super) enum Change {
    Admit(Policy, LedgerMaintenanceAdmission),
    Takeover(Policy, LedgerMaintenanceTakeover),
    Progress(Policy, LedgerMaintenanceProgress),
}

impl Change {
    pub(super) fn caller(&self) -> &Caller {
        match self {
            Self::Admit(_, r) => &r.caller,
            Self::Takeover(_, r) => &r.caller,
            Self::Progress(_, r) => &r.caller,
        }
    }
    pub(super) fn sandbox(&self) -> SandboxId {
        match self {
            Self::Admit(_, r) => r.accepted.plan().source.sandbox.id.clone(),
            Self::Takeover(_, r) => r.expected_source.sandbox.id.clone(),
            Self::Progress(_, r) => r.expected_source.sandbox.id.clone(),
        }
    }
    pub(super) fn policy(&self) -> Policy {
        match self {
            Self::Admit(p, _) | Self::Takeover(p, _) | Self::Progress(p, _) => *p,
        }
    }
}

#[async_trait::async_trait]
impl LedgerMaintenanceProgressStore for MongoSandboxStore {
    async fn progress_ledger_maintenance(
        &self,
        policy: Policy,
        request: LedgerMaintenanceProgress,
    ) -> Result<LedgerMaintenanceStateV1, Domain> {
        execute(self, Change::Progress(policy, request)).await
    }
}

#[async_trait::async_trait]
impl LedgerMaintenanceStore for MongoSandboxStore {
    async fn inspect_ledger_maintenance(
        &self,
        caller: &Caller,
        sandbox: &SandboxId,
    ) -> Result<LedgerMaintenanceInspection, Domain> {
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
            ledger_maintenance_snapshot::load(self, caller, sandbox, &mut session),
        )
        .await
        .map_err(|_| Domain::Unavailable)
        .and_then(|value| {
            value
                .map(|snapshot| snapshot.inspection)
                .map_err(ApplyError::domain)
        });
        session
            .abort_transaction()
            .await
            .map_err(|_| Domain::Unavailable)?;
        result
    }

    async fn admit_ledger_maintenance(
        &self,
        policy: Policy,
        request: LedgerMaintenanceAdmission,
    ) -> Result<LedgerMaintenanceStateV1, Domain> {
        execute(self, Change::Admit(policy, request)).await
    }

    async fn take_over_ledger_maintenance(
        &self,
        policy: Policy,
        request: LedgerMaintenanceTakeover,
    ) -> Result<LedgerMaintenanceStateV1, Domain> {
        execute(self, Change::Takeover(policy, request)).await
    }
}

async fn execute(
    store: &MongoSandboxStore,
    change: Change,
) -> Result<LedgerMaintenanceStateV1, Domain> {
    if !change.policy().enabled {
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
        let applied = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            transaction::apply(store, &change, &mut session),
        )
        .await;
        let receipt = match applied.unwrap_or(Err(ApplyError::Domain(Domain::Unavailable))) {
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
                return wait_for_reconciliation("ledger maintenance commit", async move {
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
#[path = "ledger_maintenance_tests.rs"]
mod tests;
