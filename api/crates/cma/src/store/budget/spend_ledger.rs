//! The platform spend read model over the period row: plain reads, no session,
//! no settings fence.

use layer2_core::budget::{
    BudgetError, PlatformAdmissionState, PlatformPeriodLedger, PlatformSpendLedger,
    PlatformSpendState,
};
use mongodb::bson::doc;

use super::period::{period_key, snapshot, PolicySnapshot};
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
