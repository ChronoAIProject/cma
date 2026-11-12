//! Evaluation persistence separated from policy and ledger acquisition.
use super::*;
use layer2_core::budget::PlatformBudgetPolicy;

/// Persist unchanged evaluation evidence at least once per minute.
const REFRESH_SECONDS: u64 = 60;

impl MongoSandboxStore {
    pub(super) async fn persist_evaluation(
        &self,
        platform: &PlatformBudgetPolicy,
        revision: u64,
        read: PlatformPeriodLedger,
        now: u64,
    ) -> Result<Option<Evaluation>, BudgetError> {
        let start = platform.period_bounds()?.0;
        let mut ledger = read.clone();
        let mut evaluation = ledger.evaluate(platform, revision, now)?;
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
