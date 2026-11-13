//! Shared in-memory platform spend ledger and admin router fixture.
use std::collections::BTreeMap;

use super::*;
use layer2_core::budget::{
    BudgetError, Closure, ClosureReason, CloudObservation, CloudObservationInput, CloudSpendPolicy,
    ObservationError, PlatformAdmissionState, PlatformBudgetPolicy, PlatformPeriodLedger,
    PlatformSpendLedger, PlatformSpendService, PlatformSpendState,
};

pub(crate) const SPEND: &str = "/api/v1/admin/spend";
pub(crate) const OBSERVATIONS: &str = "/api/v1/admin/spend/observations";
pub(crate) const START: u64 = 1_793_491_200; // 2026-11-01T00:00:00Z
pub(crate) const THROUGH: &str = "2026-11-02T00:00:00Z";

fn platform() -> PlatformBudgetPolicy {
    PlatformBudgetPolicy {
        period_cap: 1_000_000,
        period_start: "2026-11-01T00:00:00Z".into(),
        period_end: "2026-12-01T00:00:00Z".into(),
        warning_percent: 80,
        stop_percent: 100,
        protected_reserve: 50_000,
        lag_allowance: 50_000,
        cloud: Some(CloudSpendPolicy {
            run_rate_units_per_day: 100,
            max_observation_age_seconds: 3600,
        }),
    }
}

/// In-memory stand-in for the Mongo ledger's period and idempotency rules.
#[derive(Default)]
pub(crate) struct Ledger {
    pub(crate) observations: Mutex<BTreeMap<u64, u64>>,
    /// No cloud source: platform spend metering is not enabled.
    pub(crate) unmetered: bool,
    pub(crate) unavailable: bool,
    /// Fixed accounting evidence for the closed-view contract scenario.
    pub(crate) closed: bool,
}

impl Ledger {
    pub(crate) fn closed() -> Self {
        Self {
            observations: Mutex::new(BTreeMap::from([(START + 86_400, 1234)])),
            closed: true,
            ..Self::default()
        }
    }

    fn state(&self) -> PlatformSpendState {
        if self.unmetered {
            return PlatformSpendState {
                metering_enabled: false,
                settings_revision: 3,
                levels: None,
                committed: None,
                ledger: None,
            };
        }
        let mut ledger = PlatformPeriodLedger::open(&platform(), 3).unwrap();
        if let Some((through, units)) = self.observations.lock().unwrap().iter().next_back() {
            ledger.cloud = Some(CloudObservation {
                observed_through: *through,
                units: *units,
                recorded_at: *through,
                settings_revision: 3,
            });
        }
        if self.closed {
            ledger.evaluated_at = START;
            ledger.closures.push(Closure {
                reason: ClosureReason::MeteringUnavailable,
                since: START,
                settings_revision: 3,
                committed_at_entry: 0,
            });
        }
        PlatformSpendState {
            metering_enabled: true,
            settings_revision: 3,
            levels: platform().levels().ok(),
            // Keep replay evidence fixed instead of projecting cloud spend from the real clock.
            committed: self.closed.then_some(1234),
            ledger: Some(ledger),
        }
    }
}

#[async_trait::async_trait]
impl PlatformSpendLedger for Ledger {
    async fn admission_state(&self, _: u64) -> Result<PlatformAdmissionState, BudgetError> {
        Ok(PlatformAdmissionState::default())
    }

    async fn spend_state(&self, _: u64) -> Result<PlatformSpendState, BudgetError> {
        if self.unavailable {
            return Err(BudgetError::Unavailable);
        }
        Ok(self.state())
    }

    async fn record_cloud_observation(
        &self,
        input: CloudObservationInput,
        _: u64,
    ) -> Result<PlatformSpendState, ObservationError> {
        if self.unavailable {
            return Err(ObservationError::Unavailable);
        }
        let (start, end) = platform()
            .period_bounds()
            .map_err(|_| ObservationError::Invalid)?;
        if self.unmetered || input.observed_through < start || input.observed_through > end {
            return Err(ObservationError::Invalid);
        }
        match self
            .observations
            .lock()
            .unwrap()
            .entry(input.observed_through)
        {
            std::collections::btree_map::Entry::Occupied(entry) if *entry.get() != input.units => {
                return Err(ObservationError::Conflict)
            }
            std::collections::btree_map::Entry::Occupied(_) => {}
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(input.units);
            }
        }
        Ok(self.state())
    }
}

pub(crate) struct SpendFixture {
    pub(crate) app: Router,
    pub(crate) ledger: Arc<Ledger>,
}

pub(crate) fn spend_fixture(platform_writes: bool) -> SpendFixture {
    spend_fixture_with(platform_writes, Ledger::default())
}

pub(crate) fn spend_fixture_with(platform_writes: bool, ledger: Ledger) -> SpendFixture {
    let activity_service = ActivityService::new(Arc::new(MemoryActivity::default()));
    let settings = Arc::new(
        PlatformSettingsService::new(
            Arc::new(MemorySettings::default()),
            activity_service.clone(),
        )
        .with_model_budget_writes_enabled(true)
        .with_platform_spend_writes_enabled(platform_writes),
    );
    let sandboxes = Arc::new(InMemorySandboxStore::default());
    let profiles = Arc::new(InMemoryProfileStore::default());
    let retirement = Arc::new(StoredSandboxRetirement::new(sandboxes.clone()));
    let operators = Arc::new(layer2_core::sandbox::SandboxOperatorService::new(
        sandboxes,
        profiles,
        retirement,
        activity_service.clone(),
    ));
    let ledger = Arc::new(ledger);
    let spend = PlatformSpendService::new(ledger.clone(), activity_service.clone())
        .with_writes_enabled(settings.platform_spend_writes_enabled());
    SpendFixture {
        app: router(
            AdminState::new(settings, operators, activity_service)
                .with_platform_spend(Arc::new(spend)),
        ),
        ledger,
    }
}
