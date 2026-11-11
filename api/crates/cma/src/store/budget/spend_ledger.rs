//! The platform spend read model over the period row (plain reads, no session,
//! no settings fence), the evaluator, and the operator's cloud observations.

use std::time::{Duration, UNIX_EPOCH};

use layer2_core::budget::{
    BudgetError, CloudObservation, CloudObservationInput, Evaluation, ObservationError,
    PlatformAdmissionState, PlatformPeriodLedger, PlatformSpendLedger, PlatformSpendState,
};
use mongodb::bson::doc;
use mongodb::ClientSession;

use super::period::{
    ensure_period_row, enter_closures, load_or_open, period_key, read_snapshot, save_period,
    snapshot, PolicySnapshot,
};
use super::{decode, ApplyError, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};
use crate::store::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
    TRANSACTION_ATTEMPTS,
};

/// A row unchanged for this long is still re-persisted, so `evaluated_at`
/// shows the evaluator is alive (concurrency rule 4).
const REFRESH_SECONDS: u64 = 60;

impl MongoSandboxStore {
    /// The current settings with no owner scope; `None` without a settings record.
    pub(super) async fn platform_snapshot(&self) -> Result<Option<PolicySnapshot>, BudgetError> {
        self.budget_settings
            .find_one(doc! {"_id": "platform"})
            .await
            .map_err(|_| BudgetError::Unavailable)?
            .map(|document| snapshot(&document, None))
            .transpose()
    }

    pub(super) async fn period_ledger(
        &self,
        period_start: u64,
    ) -> Result<Option<PlatformPeriodLedger>, BudgetError> {
        self.model_budget
            .find_one(doc! {"_id": period_key(period_start)})
            .await
            .map_err(|_| BudgetError::Unavailable)?
            .as_ref()
            .map(decode)
            .transpose()
            .map_err(|_| BudgetError::Unavailable)
    }

    /// Time-driven transitions with plain reads and no settings fence. Writes
    /// only on change or after `REFRESH_SECONDS`: lifts and the evaluated
    /// revision by one revision-CAS update (a miss waits for the next sweep),
    /// entries by the guarded closure write. `entered` is narrowed to what this
    /// caller inserted. `None` without an enabled platform policy.
    pub(crate) async fn evaluate_period(
        &self,
        now: u64,
    ) -> Result<Option<Evaluation>, BudgetError> {
        let Some(current) = self.platform_snapshot().await? else {
            return Ok(None);
        };
        let Some(platform) = current.platform else {
            return Ok(None);
        };
        ensure_period_row(&self.model_budget, &platform, current.revision).await?;
        let start = platform.period_bounds()?.0;
        let read = self
            .period_ledger(start)
            .await?
            .ok_or(BudgetError::Unavailable)?;
        let mut ledger = read.clone();
        let mut evaluation = ledger.evaluate(&platform, current.revision, now)?;
        if !evaluation.changed() && now.saturating_sub(read.evaluated_at) < REFRESH_SECONDS {
            return Ok(Some(evaluation));
        }
        let lifted = mongodb::bson::serialize_to_bson(&evaluation.lifted)
            .map_err(|_| BudgetError::Unavailable)?;
        let signed = |value: u64| i64::try_from(value).map_err(|_| BudgetError::Unavailable);
        let swapped = self
            .model_budget
            .update_one(
                doc! {"_id": period_key(start), "record.revision": signed(read.revision)?},
                doc! {
                    "$pull": {"record.closures": {"reason": {"$in": lifted}}},
                    "$set": {
                        "record.settings_revision": signed(ledger.settings_revision)?,
                        "record.evaluated_at": signed(now)?,
                        "record.period_end": signed(ledger.period_end)?,
                    },
                    "$inc": {"record.revision": 1_i64},
                },
            )
            .await
            .map_err(|_| BudgetError::Unavailable)?;
        if swapped.matched_count == 0 {
            evaluation.lifted.clear();
        }
        let inserted = enter_closures(&self.model_budget, start, &evaluation.entered).await?;
        evaluation
            .entered
            .retain(|closure| inserted.contains(&closure.reason));
        Ok(Some(evaluation))
    }
}

#[async_trait::async_trait]
impl PlatformSpendLedger for MongoSandboxStore {
    async fn admission_state(&self, _now: u64) -> Result<PlatformAdmissionState, BudgetError> {
        let Some(platform) = self
            .platform_snapshot()
            .await?
            .and_then(|value| value.platform)
        else {
            return Ok(PlatformAdmissionState::default());
        };
        let ledger = self.period_ledger(platform.period_bounds()?.0).await?;
        Ok(PlatformAdmissionState {
            configured: true,
            closures: ledger.map(|ledger| ledger.closures).unwrap_or_default(),
        })
    }

    async fn spend_state(&self, now: u64) -> Result<PlatformSpendState, BudgetError> {
        let Some(current) = self.platform_snapshot().await? else {
            return Ok(PlatformSpendState {
                settings_revision: 0,
                levels: None,
                committed: None,
                ledger: None,
            });
        };
        let Some(platform) = current.platform else {
            return Ok(PlatformSpendState {
                settings_revision: current.revision,
                levels: None,
                committed: None,
                ledger: None,
            });
        };
        let ledger = self.period_ledger(platform.period_bounds()?.0).await?;
        Ok(PlatformSpendState {
            settings_revision: current.revision,
            levels: platform.levels().ok(),
            committed: ledger
                .as_ref()
                .and_then(|ledger| ledger.committed(&platform, now).ok()),
            ledger,
        })
    }

    async fn record_cloud_observation(
        &self,
        input: CloudObservationInput,
        now: u64,
    ) -> Result<PlatformSpendState, ObservationError> {
        if input.observed_through > now || input.units > (1 << 53) - 1 {
            return Err(ObservationError::Invalid);
        }
        self.observe_cloud(input, now).await?;
        self.spend_state(now)
            .await
            .map_err(|_| ObservationError::Unavailable)
    }
}

/// The observation transaction's outcome: retry from a fresh snapshot, or a
/// typed refusal.
enum Attempt {
    Retry,
    Refused(ObservationError),
}

impl From<ApplyError> for Attempt {
    fn from(error: ApplyError) -> Self {
        match error {
            ApplyError::Driver(error) if is_transient(&error) || is_duplicate_key(&error) => {
                Self::Retry
            }
            _ => Self::Refused(ObservationError::Unavailable),
        }
    }
}

impl MongoSandboxStore {
    /// Append the immutable observation row and update the period row in one
    /// transaction. An identical repeat changes nothing.
    async fn observe_cloud(
        &self,
        input: CloudObservationInput,
        now: u64,
    ) -> Result<(), ObservationError> {
        let _platform = self.platform_write_guard().await;
        for attempt in 0..TRANSACTION_ATTEMPTS {
            crate::store::contention::backoff(attempt).await;
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|_| ObservationError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| ObservationError::Unavailable)?;
            match observe_in(self, input, now, &mut session).await {
                Ok(()) => match commit_transaction(&mut session).await {
                    CommitOutcome::Committed => return Ok(()),
                    CommitOutcome::Retry(_) => continue,
                    CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                        return Err(ObservationError::Unavailable)
                    }
                },
                Err(Attempt::Retry) => {
                    let _ = session.abort_transaction().await;
                }
                Err(Attempt::Refused(error)) => {
                    let _ = session.abort_transaction().await;
                    return Err(error);
                }
            }
        }
        Err(ObservationError::Unavailable)
    }
}

async fn observe_in(
    store: &MongoSandboxStore,
    input: CloudObservationInput,
    now: u64,
    session: &mut ClientSession,
) -> Result<(), Attempt> {
    let invalid = || Attempt::Refused(ObservationError::Invalid);
    let snapshot = read_snapshot(&store.budget_settings, None, session)
        .await?
        .ok_or_else(invalid)?;
    let platform = snapshot.platform.ok_or_else(invalid)?;
    let (start, end) = platform.period_bounds().map_err(|_| invalid())?;
    if input.observed_through < start || input.observed_through > end {
        return Err(invalid());
    }
    let key = format!(
        "observation:cloud:{}",
        layer2_core::rfc3339_at(UNIX_EPOCH + Duration::from_secs(input.observed_through))
    );
    if let Some(document) = store
        .model_budget
        .find_one(doc! {"_id": &key})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
    {
        let recorded: CloudObservation = decode(&document)?;
        return if recorded.units == input.units {
            Ok(())
        } else {
            Err(Attempt::Refused(ObservationError::Conflict))
        };
    }
    let observation = CloudObservation {
        observed_through: input.observed_through,
        units: input.units,
        recorded_at: now,
        settings_revision: snapshot.revision,
    };
    let record = mongodb::bson::serialize_to_bson(&observation)
        .map_err(|_| Attempt::Refused(ObservationError::Unavailable))?;
    store
        .model_budget
        .insert_one(
            doc! {"_id": &key, "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "record": record},
        )
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    let (mut ledger, _) = load_or_open(store, &platform, snapshot.revision, session).await?;
    ledger.record_cloud(observation).map_err(|_| invalid())?;
    save_period(store, &ledger, session).await?;
    Ok(())
}
