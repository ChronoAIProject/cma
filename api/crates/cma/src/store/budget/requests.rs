//! A confirmed transaction permits one upstream send; uncertain commits never do.

use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::budget::{
    BudgetError, BudgetPolicy, ModelRequest, OwnerBudget, RequestBudget, RequestHold, RequestScope,
    RequestUsage,
};
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::{bson::doc, ClientSession};
use serde::{Deserialize, Serialize};

use super::{decode, domain, save, ApplyError, MongoSandboxStore, TurnRecord};
use crate::store::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
    TRANSACTION_ATTEMPTS,
};

mod admission;
mod settlement;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestRecord {
    hold: RequestHold,
    owner: layer2_core::SubjectId,
    policy: BudgetPolicy,
    reserved: u64,
    settled: Option<RequestUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    period_start: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    period_shard: Option<u16>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestTurnLedger {
    spent: u64,
    reserved: u64,
    calls: u64,
    bound_violated: bool,
}

fn request_key(scope: &RequestScope) -> String {
    format!("request:{}:{}", scope.agent_id.as_str(), scope.request_id)
}

fn ledger_key(scope: &RequestScope) -> String {
    format!(
        "request-turn:{}:{}",
        scope.agent_id.as_str(),
        scope.turn_sequence
    )
}

fn now(_store: &MongoSandboxStore) -> Result<u64, BudgetError> {
    #[cfg(test)]
    if let Some(clock) = &_store.request_budget_clock {
        return Ok(clock.load(std::sync::atomic::Ordering::SeqCst));
    }
    layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339()).ok_or(BudgetError::Unavailable)
}

enum Operation<'a> {
    Reserve(&'a RequestScope, &'a ModelRequest),
    Settle(&'a RequestHold, RequestUsage),
}

impl MongoSandboxStore {
    async fn request_transaction(
        &self,
        operation: Operation<'_>,
    ) -> Result<Option<RequestHold>, BudgetError> {
        let _platform = match &operation {
            Operation::Reserve(..) => self.platform_write_guard().await,
            Operation::Settle(..) => self.platform_settlement_guard().await,
        };
        for attempt in 0..TRANSACTION_ATTEMPTS {
            crate::store::contention::backoff(attempt).await;
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|_| BudgetError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| BudgetError::Unavailable)?;
            let result = match &operation {
                Operation::Reserve(scope, request) => {
                    admission::reserve(self, scope, request, &mut session).await
                }
                Operation::Settle(hold, usage) => {
                    settlement::settle(self, hold, *usage, &mut session)
                        .await
                        .map(|()| None)
                }
            };
            let permit = match result {
                Ok(permit) => permit,
                Err(ApplyError::Domain(SandboxStoreError::Budget(reason))) => {
                    let _ = session.abort_guarded().await;
                    return Err(reason);
                }
                Err(ApplyError::Driver(error))
                    if is_transient(&error) || is_duplicate_key(&error) =>
                {
                    let _ = session.abort_guarded().await;
                    continue;
                }
                Err(_) => {
                    let _ = session.abort_guarded().await;
                    return Err(BudgetError::Unavailable);
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(permit),
                CommitOutcome::Retry(_) => continue,
                // Do not reconcile into another send permit: the holder cannot
                // prove that a previous caller has not already used that permit.
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(BudgetError::Unavailable)
                }
            }
        }
        Err(BudgetError::Unavailable)
    }
}

#[async_trait::async_trait]
impl RequestBudget for MongoSandboxStore {
    async fn reserve_request(
        &self,
        scope: &RequestScope,
        request: &ModelRequest,
    ) -> Result<Option<RequestHold>, BudgetError> {
        if scope.turn_sequence == 0
            || scope.request_id.is_empty()
            || scope.request_id.len() > 160
            || !scope
                .request_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b':' | b'_'))
        {
            return Err(BudgetError::Unavailable);
        }
        self.request_transaction(Operation::Reserve(scope, request))
            .await
    }

    async fn settle_request(
        &self,
        hold: &RequestHold,
        usage: RequestUsage,
    ) -> Result<(), BudgetError> {
        self.request_transaction(Operation::Settle(hold, usage))
            .await
            .map(|_| ())
    }
}
