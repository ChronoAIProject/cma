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

#[path = "evaluation.rs"]
mod evaluation;

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
        self.persist_evaluation(&platform, current.revision, read, now)
            .await
    }
}

#[async_trait::async_trait]
impl PlatformSpendLedger for MongoSandboxStore {
    async fn admission_state(&self, now: u64) -> Result<PlatformAdmissionState, BudgetError> {
        let Some(PolicySnapshot {
            platform: Some(platform),
            revision,
            ..
        }) = self.platform_snapshot().await?
        else {
            return Ok(PlatformAdmissionState::default());
        };
        let ledger = match self.period_ledger(platform.period_bounds()?.0).await? {
            Some(ledger) => ledger,
            None => PlatformPeriodLedger::open(&platform, revision)?,
        };
        ledger.admission_state(&platform, revision, now)
    }

    async fn spend_state(&self, now: u64) -> Result<PlatformSpendState, BudgetError> {
        let Some(current) = self.platform_snapshot().await? else {
            return Ok(PlatformSpendState {
                metering_enabled: false,
                settings_revision: 0,
                levels: None,
                committed: None,
                ledger: None,
            });
        };
        let Some(platform) = current.platform else {
            return Ok(PlatformSpendState {
                metering_enabled: false,
                settings_revision: current.revision,
                levels: None,
                committed: None,
                ledger: None,
            });
        };
        let ledger = self.period_ledger(platform.period_bounds()?.0).await?;
        Ok(PlatformSpendState {
            metering_enabled: true,
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
    // Period-scoped: adjacent periods share their boundary instant, and each
    // may record its own observation there.
    let at = |seconds| layer2_core::rfc3339_at(UNIX_EPOCH + Duration::from_secs(seconds));
    let key = format!(
        "observation:cloud:{}:{}",
        at(start),
        at(input.observed_through)
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
