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
        let mut ledger = read.clone();
        let mut evaluation = ledger.evaluate(platform, revision, now)?;
        if !evaluation.changed() && now.saturating_sub(read.evaluated_at) < REFRESH_SECONDS {
            return Ok(Some(evaluation));
        }
        if super::super::transitions::persist(
            &self.budget_settings,
            &self.model_budget,
            revision,
            Some(read.revision),
            &ledger,
        )
        .await?
        {
            evaluation
                .entered
                .iter()
                .for_each(super::super::period::alert);
        } else {
            // Every transition was derived from the same stale evidence. A
            // failed lift CAS must never fall through to an unfenced entry.
            evaluation = Evaluation::default();
        }
        Ok(Some(evaluation))
    }
}

#[cfg(test)]
#[path = "evaluation_tests.rs"]
mod tests;
