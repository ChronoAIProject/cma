//! Operator platform spend routes behind the third write gate.
use std::collections::BTreeMap;

use super::*;
use layer2_core::budget::{
    BudgetError, CloudObservation, CloudObservationInput, ObservationError, PlatformAdmissionState,
    PlatformBudgetPolicy, PlatformPeriodLedger, PlatformSpendLedger, PlatformSpendService,
    PlatformSpendState,
};

const SPEND: &str = "/api/v1/admin/spend";
const OBSERVATIONS: &str = "/api/v1/admin/spend/observations";
const START: u64 = 1_793_491_200; // 2026-11-01T00:00:00Z
const THROUGH: &str = "2026-11-02T00:00:00Z";

fn platform() -> PlatformBudgetPolicy {
    PlatformBudgetPolicy {
        period_cap: 1_000_000,
        period_start: "2026-11-01T00:00:00Z".into(),
        period_end: "2026-12-01T00:00:00Z".into(),
        warning_percent: 80,
        stop_percent: 100,
        protected_reserve: 50_000,
        lag_allowance: 50_000,
        cloud: None,
    }
}

/// In-memory stand-in for the Mongo ledger's period and idempotency rules.
#[derive(Default)]
struct Ledger {
    observations: Mutex<BTreeMap<u64, u64>>,
    /// No cloud source: platform spend metering is not enabled.
    unmetered: bool,
}

impl Ledger {
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
        PlatformSpendState {
            metering_enabled: true,
            settings_revision: 3,
            levels: platform().levels().ok(),
            committed: None,
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
        Ok(self.state())
    }

    async fn record_cloud_observation(
        &self,
        input: CloudObservationInput,
        _: u64,
    ) -> Result<PlatformSpendState, ObservationError> {
        if input.observed_through < START {
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

struct SpendFixture {
    app: Router,
    ledger: Arc<Ledger>,
}

fn spend_fixture(platform_writes: bool) -> SpendFixture {
    spend_fixture_with(platform_writes, Ledger::default())
}

fn spend_fixture_with(platform_writes: bool, ledger: Ledger) -> SpendFixture {
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

fn observation(units: Value, observed_through: &str) -> Value {
    json!({"source": "cloud", "observed_through": observed_through, "units": units})
}

#[tokio::test]
async fn get_spend_returns_state_for_operators_and_403_for_others() {
    let fixture = spend_fixture(true);
    for grants in [vec![], vec![Capability::ReadAllSandboxes]] {
        let denied = send(&fixture.app, Method::GET, SPEND, None, grants).await;
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    }
    let read = send(&fixture.app, Method::GET, SPEND, None, ADMIN).await;
    assert_eq!(read.status(), StatusCode::OK);
    let body = response_json(read).await;
    assert_eq!(body["metering_enabled"], true);
    assert_eq!(body["settings_revision"], 3);
    assert_eq!(
        body["levels"],
        json!({"warning": 800_000, "stop": 1_000_000, "disposable": 900_000})
    );
    assert_eq!(body["ledger"]["period_start"], "2026-11-01T00:00:00Z");
    assert_eq!(body["ledger"]["closures"], json!([]));
    assert!(body["ledger"].get("cloud").is_none());
}

#[tokio::test]
async fn post_observation_validates_time_bounds_and_is_idempotent() {
    let closed = spend_fixture(false);
    let refused = send(
        &closed.app,
        Method::POST,
        OBSERVATIONS,
        Some(observation(json!(10), THROUGH)),
        ADMIN,
    )
    .await;
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response_json(refused).await["code"],
        "model_budget_writes_unavailable"
    );
    let fixture = spend_fixture(true);
    let denied = send(
        &fixture.app,
        Method::POST,
        OBSERVATIONS,
        Some(observation(json!(10), THROUGH)),
        [Capability::ReadAllSandboxes],
    )
    .await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    for _ in 0..2 {
        let saved = send(
            &fixture.app,
            Method::POST,
            OBSERVATIONS,
            Some(observation(json!(10), THROUGH)),
            ADMIN,
        )
        .await;
        assert_eq!(saved.status(), StatusCode::OK);
        let body = response_json(saved).await;
        assert_eq!(body["ledger"]["cloud"]["observed_through"], THROUGH);
        assert_eq!(body["ledger"]["cloud"]["units"], 10);
    }
    let conflict = send(
        &fixture.app,
        Method::POST,
        OBSERVATIONS,
        Some(observation(json!(11), THROUGH)),
        ADMIN,
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let conflict = response_json(conflict).await;
    assert_eq!(conflict["code"], "invalid_request");
    assert_eq!(
        conflict["detail"],
        "observation conflicts with a recorded value"
    );
    let unsafe_units = json!((1_u64 << 53));
    for body in [
        observation(json!(10), "2100-01-01T00:00:00Z"),
        observation(json!(10), "2026-10-31T23:59:59Z"),
        observation(json!(1.5), THROUGH),
        observation(json!(-1), THROUGH),
        observation(unsafe_units, THROUGH),
        observation(json!(10), "yesterday"),
        json!({"source": "model_provider", "observed_through": THROUGH, "units": 10}),
        json!({"source": "cloud", "observed_through": THROUGH, "units": 10, "extra": true}),
    ] {
        let response = send(
            &fixture.app,
            Method::POST,
            OBSERVATIONS,
            Some(body.clone()),
            ADMIN,
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{body}"
        );
        assert_eq!(response_json(response).await["code"], "invalid_request");
    }
    assert_eq!(fixture.ledger.observations.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn settings_view_reports_platform_spend_writes_available() {
    for open in [false, true] {
        let fixture = spend_fixture(open);
        let settings = response_json(
            send(
                &fixture.app,
                Method::GET,
                "/api/v1/admin/settings",
                None,
                ADMIN,
            )
            .await,
        )
        .await;
        assert_eq!(settings["platform_spend_writes_available"], open);
        assert_eq!(settings["model_budget_writes_available"], true);
    }
}

#[tokio::test]
async fn cloud_policy_fields_round_trip_through_the_settings_body() {
    let fixture = spend_fixture(true);
    let mut policy = scoped_policy();
    policy["platform"]["cloud"] =
        json!({"run_rate_units_per_day": 100, "max_observation_age_seconds": 3600});
    let saved = send(
        &fixture.app,
        Method::PATCH,
        "/api/v1/admin/settings",
        Some(json!({"expected_revision": 0, "model_budget": policy.clone()})),
        ADMIN,
    )
    .await;
    assert_eq!(saved.status(), StatusCode::OK);
    assert_eq!(
        response_json(saved).await["settings"]["model_budget"],
        policy
    );
    let closed = spend_fixture(false);
    let refused = send(
        &closed.app,
        Method::PATCH,
        "/api/v1/admin/settings",
        Some(json!({"expected_revision": 0, "model_budget": policy})),
        ADMIN,
    )
    .await;
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn get_spend_reports_metering_off_without_a_cloud_source() {
    let fixture = spend_fixture_with(
        true,
        Ledger {
            unmetered: true,
            ..Ledger::default()
        },
    );
    let body = response_json(send(&fixture.app, Method::GET, SPEND, None, ADMIN).await).await;
    assert_eq!(body["metering_enabled"], false);
    assert_eq!(body["settings_revision"], 3);
    for absent in ["levels", "committed", "ledger"] {
        assert!(body.get(absent).is_none(), "{absent}");
    }
}
