use super::*;

#[tokio::test]
async fn every_spend_control_roundtrips_with_revision_checks_and_confidential_audit() {
    let fixture = budget_fixture(true);
    let mut settings = scoped_policy();
    settings["default_policy"] = settings["owners"][0]["policy"].clone();
    let saved = send(
        &fixture.app,
        Method::PATCH,
        PATH,
        Some(json!({"expected_revision":0,"model_budget":settings})),
        ADMIN,
    )
    .await;
    assert_eq!(saved.status(), StatusCode::OK);
    assert_eq!(
        response_json(saved).await["settings"]["model_budget"],
        settings
    );
    let current = response_json(send(&fixture.app, Method::GET, PATH, None, ADMIN).await).await;
    assert_eq!(current["settings"]["model_budget"], settings);
    assert_eq!(
        send(
            &fixture.app,
            Method::PATCH,
            PATH,
            Some(json!({"expected_revision":0,"model_budget":settings})),
            ADMIN
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    for record in fixture.activity.records.lock().unwrap().iter() {
        let audit = serde_json::to_string(record).unwrap();
        for value in [
            "period_cap",
            "protected_reserve",
            "lag_allowance",
            "owner_day_limit",
            "budget-test-owner",
        ] {
            assert!(!audit.contains(value));
        }
    }
}

#[tokio::test]
async fn invalid_or_incomplete_controls_never_mutate_the_settings_revision() {
    let fixture = budget_fixture(true);
    let mut invalids = Vec::new();
    for (path, value) in [
        ("/platform/warning_percent", json!(100)),
        ("/platform/stop_percent", json!(101)),
        ("/platform/protected_reserve", json!(u64::from(u32::MAX))),
        ("/platform/lag_allowance", json!(u64::MAX)),
        ("/platform/period_end", json!("2026-10-01T00:00:00Z")),
        ("/platform/period_start", json!("invalid")),
        ("/owners/0/policy/max_calls_per_turn", json!(0)),
        ("/owners/0/policy/day_timezone", json!("invalid/timezone")),
    ] {
        let mut policy = scoped_policy();
        *policy.pointer_mut(path).unwrap() = value;
        invalids.push(policy);
    }
    invalids.push(json!({"enabled":true,"owners":[]}));
    let mut incomplete = scoped_policy();
    incomplete["platform"]
        .as_object_mut()
        .unwrap()
        .remove("protected_reserve");
    invalids.push(incomplete);
    for policy in invalids {
        let response = send(
            &fixture.app,
            Method::PATCH,
            PATH,
            Some(json!({"expected_revision":0,"model_budget":policy})),
            ADMIN,
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(!response_json(response)
            .await
            .to_string()
            .contains("budget-test-owner"));
    }
    let current = response_json(send(&fixture.app, Method::GET, PATH, None, ADMIN).await).await;
    assert_eq!(current["revision"], 0);
    assert_eq!(
        current["settings"]["model_budget"],
        json!({"enabled":false,"owners":[]})
    );
}

#[tokio::test]
async fn disabling_retains_explicit_values_without_enforcing_owner_policy() {
    let fixture = budget_fixture(true);
    let mut policy = scoped_policy();
    policy["enabled"] = json!(false);
    let response = send(
        &fixture.app,
        Method::PATCH,
        PATH,
        Some(json!({"expected_revision":0,"model_budget":policy})),
        ADMIN,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await["settings"]["model_budget"],
        policy
    );
}
