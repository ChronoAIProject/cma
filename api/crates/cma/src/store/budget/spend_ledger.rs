//! The platform spend read model over the period row: plain reads, no session,
//! no settings fence.

use layer2_core::budget::{
    BudgetError, Evaluation, PlatformAdmissionState, PlatformPeriodLedger, PlatformSpendLedger,
    PlatformSpendState,
};
use mongodb::bson::doc;

use super::period::{ensure_period_row, enter_closures, period_key, snapshot, PolicySnapshot};

/// A row unchanged for this long is still re-persisted, so `evaluated_at`
/// shows the evaluator is alive (concurrency rule 4).
const REFRESH_SECONDS: u64 = 60;
use super::{decode, MongoSandboxStore};

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
}
