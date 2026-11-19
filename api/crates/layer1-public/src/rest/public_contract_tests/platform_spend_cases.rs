//! Real spend handlers and service, with synthetic accounting persistence.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use layer2_core::budget::{
    BudgetError, Closure, ClosureReason, CloudObservation, CloudObservationInput, ObservationError,
    PlatformAdmissionState, PlatformBudgetPolicy, PlatformPeriodLedger, PlatformSpendLedger,
    PlatformSpendService, PlatformSpendState,
};
use layer2_core::profiles::store::InMemoryProfileStore;
use layer2_core::sandbox::{
    store::InMemorySandboxStore, SandboxOperatorService, StoredSandboxRetirement,
};
use layer2_core::{ActivityService, Authorization, Capability, PlatformSettingsService};

use super::super::admin::{
    self,
    tests::{MemoryActivity, MemorySettings},
    AdminState,
};
use super::Scenario;

const THROUGH: u64 = 1_767_312_000; // 2026-01-02T00:00:00Z

fn policy() -> PlatformBudgetPolicy {
    PlatformBudgetPolicy {
        period_cap: 100,
        period_start: "2026-01-01T00:00:00Z".into(),
        period_end: "2026-02-01T00:00:00Z".into(),
        warning_percent: 80,
        stop_percent: 100,
        protected_reserve: 5,
        lag_allowance: 5,
        cloud: None,
    }
}

struct Ledger {
    kind: String,
    observations: Mutex<BTreeMap<u64, u64>>,
}

impl Ledger {
    fn state(&self) -> PlatformSpendState {
        let mut state = PlatformSpendState {
            metering_enabled: self.kind != "disabled",
            settings_revision: 3,
            levels: None,
            committed: None,
            ledger: None,
        };
        if !state.metering_enabled {
            return state;
        }
        let mut ledger = PlatformPeriodLedger::open(&policy(), 3).unwrap();
        if let Some((&observed_through, &units)) =
            self.observations.lock().unwrap().last_key_value()
        {
            ledger
                .record_cloud(CloudObservation {
                    observed_through,
                    units,
                    recorded_at: observed_through + 1,
                    settings_revision: 3,
                })
                .unwrap();
            ledger.evaluated_at = observed_through + 1;
            state.committed = Some(units);
        }
        let closure = match self.kind.as_str() {
            "warning" => Some(ClosureReason::SpendWarning),
            "stop" => Some(ClosureReason::SpendStop),
            "unmetered" => Some(ClosureReason::MeteringUnavailable),
            _ => None,
        };
        if let Some(reason) = closure {
            ledger.closures.push(Closure {
                reason,
                since: THROUGH,
                settings_revision: 2,
                committed_at_entry: 90,
            });
            ledger.evaluated_at = THROUGH;
        }
        state.levels = Some(policy().levels().unwrap());
        state.ledger = Some(ledger);
        state
    }
}

#[async_trait]
impl PlatformSpendLedger for Ledger {
    async fn admission_state(&self, _: u64) -> Result<PlatformAdmissionState, BudgetError> {
        Ok(PlatformAdmissionState::default())
    }

    async fn spend_state(&self, _: u64) -> Result<PlatformSpendState, BudgetError> {
        if self.kind == "unavailable" {
            return Err(BudgetError::Unavailable);
        }
        Ok(self.state())
    }

    async fn record_cloud_observation(
        &self,
        input: CloudObservationInput,
        _: u64,
    ) -> Result<PlatformSpendState, ObservationError> {
        if self.kind == "unavailable" {
            return Err(ObservationError::Unavailable);
        }
        let mut period = PlatformPeriodLedger::open(&policy(), 3).unwrap();
        period
            .record_cloud(CloudObservation {
                observed_through: input.observed_through,
                units: input.units,
                recorded_at: input.observed_through + 1,
                settings_revision: 3,
            })
            .map_err(|_| ObservationError::Invalid)?;
        {
            let mut observations = self.observations.lock().unwrap();
            match observations.entry(input.observed_through) {
                std::collections::btree_map::Entry::Occupied(entry) => {
                    if *entry.get() != input.units {
                        return Err(ObservationError::Conflict);
                    }
                }
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(input.units);
                }
            }
        }
        Ok(self.state())
    }
}

pub(super) fn scenario(name: &str) -> Option<Scenario> {
    let kind = name.strip_prefix("platform_spend_")?;
    assert!(
        matches!(
            kind,
            "operator"
                | "anonymous"
                | "forbidden"
                | "disabled"
                | "writes_closed"
                | "foundation_closed"
                | "unavailable"
                | "absent"
                | "absent_forbidden"
                | "warning"
                | "stop"
                | "unmetered"
                | "bounds"
        ),
        "unknown spend scenario {kind}"
    );
    let activity = ActivityService::new(Arc::new(MemoryActivity::default()));
    let settings = Arc::new(
        PlatformSettingsService::new(Arc::new(MemorySettings::default()), activity.clone())
            .with_model_budget_writes_enabled(kind != "foundation_closed")
            .with_platform_spend_writes_enabled(kind != "writes_closed"),
    );
    let writes_enabled = settings.platform_spend_writes_enabled();
    let sandboxes = Arc::new(InMemorySandboxStore::default());
    let operators = Arc::new(SandboxOperatorService::new(
        sandboxes.clone(),
        Arc::new(InMemoryProfileStore::default()),
        Arc::new(StoredSandboxRetirement::new(sandboxes)),
        activity.clone(),
    ));
    let mut state = AdminState::new(settings, operators, activity.clone());
    if !kind.starts_with("absent") {
        let ledger = Arc::new(Ledger {
            kind: kind.into(),
            observations: Mutex::new(BTreeMap::new()),
        });
        state = state.with_platform_spend(Arc::new(
            PlatformSpendService::new(ledger, activity).with_writes_enabled(writes_enabled),
        ));
    }
    Some(Scenario {
        router: admin::router(state),
        subject: (kind != "anonymous").then_some("operator-fixture"),
        authorization: Authorization::new(
            (!kind.ends_with("forbidden")).then_some(Capability::ConfigurePlatform),
        ),
        symbols: BTreeMap::new(),
    })
}
