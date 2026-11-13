//! Operator access to the platform spend period: `ConfigurePlatform`
//! authority, a reader-before-write gate, and confidential audit.

use std::sync::Arc;

use super::{CloudObservationInput, ObservationError, PlatformSpendLedger, PlatformSpendState};
use crate::{
    ActivityAction, ActivityOutcome, ActivityRecord, ActivityService, ActivityTarget, Caller,
    Capability, CoreError, DetailKey,
};

const SPEND_TARGET: &str = "platform_spend";

pub struct PlatformSpendService {
    ledger: Arc<dyn PlatformSpendLedger>,
    activity: Arc<ActivityService>,
    writes_enabled: bool,
}

impl PlatformSpendService {
    pub fn new(ledger: Arc<dyn PlatformSpendLedger>, activity: Arc<ActivityService>) -> Self {
        Self {
            ledger,
            activity,
            writes_enabled: false,
        }
    }

    /// Open only once every serving and rollback reader understands the shape.
    pub fn with_writes_enabled(mut self, enabled: bool) -> Self {
        self.writes_enabled = enabled;
        self
    }

    pub async fn read(&self, caller: &Caller) -> Result<PlatformSpendState, CoreError> {
        caller.require(Capability::ConfigurePlatform)?;
        self.ledger
            .spend_state(now())
            .await
            .map_err(|error| CoreError::RuntimeUnavailable(error.to_string()))
    }

    /// The audit carries the operation and settings revision, never units.
    pub async fn record_cloud_observation(
        &self,
        caller: &Caller,
        input: CloudObservationInput,
    ) -> Result<PlatformSpendState, CoreError> {
        caller.require(Capability::ConfigurePlatform)?;
        if !self.writes_enabled {
            return Err(CoreError::ModelBudgetWritesUnavailable);
        }
        let state = self
            .ledger
            .record_cloud_observation(input, now())
            .await
            .map_err(|error| match error {
                ObservationError::Invalid | ObservationError::Conflict => {
                    CoreError::InvalidRequest(error.to_string())
                }
                ObservationError::Unavailable => CoreError::RuntimeUnavailable(error.to_string()),
            })?;
        let mut record = ActivityRecord::new(ActivityAction::Configure, ActivityOutcome::Succeeded)
            .by(caller.subject.clone())
            .on(ActivityTarget::Resource(SPEND_TARGET.into()));
        record
            .detail
            .record(DetailKey::Revision, state.settings_revision.to_string());
        self.activity
            .record(&record)
            .await
            .map_err(|error| CoreError::RuntimeUnavailable(error.to_string()))?;
        Ok(state)
    }
}

fn now() -> u64 {
    crate::seconds_from_rfc3339(&crate::now_rfc3339()).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[path = "spend_service_tests.rs"]
mod tests;
