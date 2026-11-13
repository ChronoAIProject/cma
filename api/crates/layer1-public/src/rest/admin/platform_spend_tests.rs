//! Operator platform spend routes behind the third write gate.

use super::*;
use crate::rest::admin::tests::spend_test_support::*;

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

#[tokio::test]
async fn spend_ledger_refuses_observations_while_metering_is_off() {
    use layer2_core::budget::{CloudObservationInput, ObservationError, PlatformSpendLedger};
    let ledger = Ledger {
        unmetered: true,
        ..Ledger::default()
    };
    let input = CloudObservationInput {
        observed_through: START + 86_400,
        units: 1234,
    };
    assert_eq!(
        ledger
            .record_cloud_observation(input, input.observed_through)
            .await,
        Err(ObservationError::Invalid)
    );
    assert!(ledger.observations.lock().unwrap().is_empty());
}

#[tokio::test]
async fn spend_ledger_observation_period_bounds_are_inclusive() {
    use layer2_core::budget::{CloudObservationInput, ObservationError, PlatformSpendLedger};
    let ledger = Ledger::default();
    let end = layer2_core::seconds_from_rfc3339("2026-12-01T00:00:00Z").unwrap();
    for observed_through in [START - 1, end + 1] {
        assert_eq!(
            ledger
                .record_cloud_observation(
                    CloudObservationInput {
                        observed_through,
                        units: 1234
                    },
                    end + 1
                )
                .await,
            Err(ObservationError::Invalid)
        );
    }
    assert!(ledger.observations.lock().unwrap().is_empty());
    for observed_through in [START, end] {
        assert!(ledger
            .record_cloud_observation(
                CloudObservationInput {
                    observed_through,
                    units: 1234
                },
                end + 1
            )
            .await
            .is_ok());
    }
    assert_eq!(ledger.observations.lock().unwrap().len(), 2);
}

#[test]
fn spend_closure_reason_projection_preserves_core_wire_bytes() {
    use crate::rest::admin::spend::SpendClosureReason;
    use layer2_core::budget::ClosureReason;
    for reason in [
        ClosureReason::SpendWarning,
        ClosureReason::SpendStop,
        ClosureReason::MeteringUnavailable,
    ] {
        assert_eq!(
            serde_json::to_vec(&SpendClosureReason::from(reason)).unwrap(),
            serde_json::to_vec(&reason).unwrap()
        );
    }
}
