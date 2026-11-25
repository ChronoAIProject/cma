//! Cached STOP observations schedule work. Only this transactional evidence
//! authorizes a new cancellation receipt or a durable platform pause.
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::budget::{BudgetError, ClosureReason};
use mongodb::bson::doc;
use mongodb::ClientSession;

use super::period::{period_key, read_snapshot};
use super::{domain, ApplyError, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};
use crate::store::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
    TRANSACTION_ATTEMPTS,
};

pub(in crate::store) async fn fence_stop(
    store: &MongoSandboxStore,
    session: &mut ClientSession,
) -> Result<bool, ApplyError> {
    let Some(snapshot) = read_snapshot(&store.budget_settings, None, session).await? else {
        return Ok(false);
    };
    let Some(platform) = snapshot.platform else {
        return Ok(false);
    };
    let start = platform.period_bounds().map_err(domain)?.0;
    let Some(state) = super::period_storage::read_root(&store.model_budget, start, session).await?
    else {
        return Ok(false);
    };
    let ledger = &state.ledger;
    if !ledger
        .closures
        .iter()
        .any(|closure| closure.reason == ClosureReason::SpendStop)
    {
        return Ok(false);
    }
    let signed = |value| i64::try_from(value).map_err(|_| domain(BudgetError::Unavailable));
    // Real writes are necessary: snapshot reads alone allow a lift or policy
    // disable to commit while this transaction persists an obsolete decision.
    let settings = store
        .budget_settings
        .update_one(
            doc! {"_id": "platform", "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "revision": signed(snapshot.revision)?},
            doc! {"$inc": {"platform_stop_fence": 1_i64}},
        )
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    if settings.matched_count != 1 {
        return Ok(false);
    }
    let period = store
        .model_budget
        .update_one(
            doc! {"_id": period_key(start), "record.revision": signed(ledger.revision)?},
            doc! {"$inc": {"platform_stop_fence": 1_i64}},
        )
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    Ok(period.matched_count == 1)
}

impl MongoSandboxStore {
    pub(super) async fn record_platform_stop(&self, key: &str) -> Result<bool, BudgetError> {
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
            let result = self.platform_stop_in(key, &mut session).await;
            match result {
                Ok(true) => match commit_transaction(&mut session).await {
                    CommitOutcome::Committed => return Ok(true),
                    CommitOutcome::Retry(_) => continue,
                    CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                        return Err(BudgetError::Unavailable);
                    }
                },
                result => {
                    if let Err(error) = session.abort_guarded().await {
                        tracing::warn!(event = "platform_stop_abort", error = %error,
                            "could not confirm platform stop receipt abort");
                    }
                    match result {
                        Ok(false) => return Ok(false),
                        Err(ApplyError::Driver(error))
                            if is_transient(&error) || is_duplicate_key(&error) =>
                        {
                            continue
                        }
                        _ => return Err(BudgetError::Unavailable),
                    }
                }
            }
        }
        Err(BudgetError::Unavailable)
    }

    async fn platform_stop_in(
        &self,
        key: &str,
        session: &mut ClientSession,
    ) -> Result<bool, ApplyError> {
        if !fence_stop(self, session).await? {
            return Ok(false);
        }
        let reason = mongodb::bson::serialize_to_bson(&BudgetError::PlatformExhausted)
            .map_err(|_| domain(BudgetError::Unavailable))?;
        self.model_budget
            .update_one(
                doc! {"_id": key},
                doc! {"$setOnInsert": {
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION, "record": reason,
                }},
            )
            .upsert(true)
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?;
        Ok(true)
    }
}
