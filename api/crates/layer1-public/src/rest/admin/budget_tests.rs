use super::super::*;

#[path = "budget_controls_tests.rs"]
mod controls;
#[path = "platform_spend_tests.rs"]
mod platform_spend;
#[path = "request_budget_rollout_tests.rs"]
mod request_rollout;
#[path = "budget_rollout_tests.rs"]
mod rollout;

const PATH: &str = "/api/v1/admin/settings";
const ADMIN: [Capability; 1] = [Capability::ConfigurePlatform];

fn scoped_policy() -> Value {
    let unit = u64::from(u8::MAX);
    json!({"enabled": true, "platform": {
        "period_cap": unit * unit * unit,
        "period_start":"2026-11-01T00:00:00Z", "period_end":"2026-12-01T00:00:00Z",
        "warning_percent":80,"stop_percent":100,"protected_reserve":0,"lag_allowance":0
    }, "owners": [{"owner": "budget-test-owner", "policy": {
        "owner_day_limit": unit * unit, "turn_limit": unit,
        "max_calls_per_turn":unit,"day_timezone":"Asia/Singapore",
        "input_token_units": 1, "output_token_units": 1,
        "max_unobserved_units": 1, "usage_timeout_seconds": 60
    }}]})
}

#[tokio::test]
async fn model_budget_settings_require_admin_cas_and_keep_values_out_of_audit() {
    let fixture = budget_fixture(true);
    let body = json!({"expected_revision": 0, "model_budget": scoped_policy()});
    for grants in [
        vec![],
        vec![Capability::UseOwnSandboxes],
        vec![Capability::ReadAllSandboxes],
    ] {
        assert_eq!(
            send(
                &fixture.app,
                Method::PATCH,
                PATH,
                Some(body.clone()),
                grants
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
    }
    let saved = send(&fixture.app, Method::PATCH, PATH, Some(body.clone()), ADMIN).await;
    assert_eq!(saved.status(), StatusCode::OK);
    let saved = response_json(saved).await;
    assert_eq!(saved["settings"]["model_budget"], scoped_policy());
    assert_eq!(
        send(&fixture.app, Method::PATCH, PATH, Some(body), ADMIN)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    for record in fixture.activity.records.lock().unwrap().iter() {
        let audit = serde_json::to_string(record).unwrap();
        assert!(!audit.contains("owner_day_limit"));
        assert!(!audit.contains("budget-test-owner"));
    }
}

#[tokio::test]
async fn legacy_replace_preserves_caps_and_explicit_empty_scope_disables_them() {
    let fixture = budget_fixture(true);
    let saved = response_json(
        send(
            &fixture.app,
            Method::PATCH,
            PATH,
            Some(json!({"expected_revision": 0, "model_budget": scoped_policy()})),
            ADMIN,
        )
        .await,
    )
    .await;
    let mut settings = saved["settings"].clone();
    settings.as_object_mut().unwrap().remove("model_budget");
    let replaced = send(
        &fixture.app,
        Method::PUT,
        PATH,
        Some(json!({"expected_revision": 1, "settings": settings})),
        ADMIN,
    )
    .await;
    assert_eq!(replaced.status(), StatusCode::OK);
    assert_eq!(
        response_json(replaced).await["settings"]["model_budget"],
        scoped_policy()
    );
    let disabled = send(
        &fixture.app,
        Method::PATCH,
        PATH,
        Some(json!({"expected_revision": 2, "model_budget": {"owners": []}})),
        ADMIN,
    )
    .await;
    assert_eq!(disabled.status(), StatusCode::OK);
    assert_eq!(
        response_json(disabled).await["settings"]["model_budget"],
        json!({"enabled": false, "owners": []})
    );
}

#[tokio::test]
async fn malformed_budget_policies_are_confidential_and_never_saved() {
    let fixture = budget_fixture(true);
    let mut duplicate = scoped_policy();
    let owner = duplicate["owners"][0].clone();
    duplicate["owners"].as_array_mut().unwrap().push(owner);
    let mut missing_liability = scoped_policy();
    missing_liability["owners"][0]["policy"]
        .as_object_mut()
        .unwrap()
        .remove("max_unobserved_units");
    for invalid in [Value::Null, json!({}), duplicate, missing_liability] {
        let response = send(
            &fixture.app,
            Method::PATCH,
            PATH,
            Some(json!({"expected_revision": 0, "model_budget": invalid})),
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
}

#[tokio::test]
async fn request_model_bounds_use_the_same_private_admin_validation_and_roundtrip() {
    let fixture = budget_fixture_with_gates(true, true, Arc::new(MemorySettings::default()));
    let mut policy = scoped_policy();
    policy["request_models"] = json!([{"model":"approved", "input_overhead_tokens": 1}]);
    let saved = send(
        &fixture.app,
        Method::PATCH,
        PATH,
        Some(json!({"expected_revision":0,"model_budget":policy})),
        ADMIN,
    )
    .await;
    assert_eq!(saved.status(), StatusCode::OK);
    assert_eq!(
        response_json(saved).await["settings"]["model_budget"],
        policy
    );
    policy["request_models"][0]["input_overhead_tokens"] = json!(0);
    assert_eq!(
        send(
            &fixture.app,
            Method::PATCH,
            PATH,
            Some(json!({"expected_revision":1,"model_budget":policy})),
            ADMIN
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

struct BudgetFixture {
    app: Router,
    activity: Arc<MemoryActivity>,
}

fn budget_fixture(writes_enabled: bool) -> BudgetFixture {
    budget_fixture_with_store(writes_enabled, Arc::new(MemorySettings::default()))
}

fn budget_fixture_with_store(writes_enabled: bool, store: Arc<MemorySettings>) -> BudgetFixture {
    budget_fixture_with_gates(writes_enabled, false, store)
}

fn budget_fixture_with_gates(
    writes_enabled: bool,
    request_writes_enabled: bool,
    store: Arc<MemorySettings>,
) -> BudgetFixture {
    let activity = Arc::new(MemoryActivity::default());
    let activity_service = ActivityService::new(activity.clone());
    let settings = Arc::new(
        PlatformSettingsService::new(store, activity_service.clone())
            .with_model_budget_writes_enabled(writes_enabled)
            .with_model_request_budget_writes_enabled(request_writes_enabled),
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
    BudgetFixture {
        app: router(AdminState::new(settings, operators, activity_service)),
        activity,
    }
}
