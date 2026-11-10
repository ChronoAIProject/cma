use super::super::*;

const PATH: &str = "/api/v1/admin/settings";
const ADMIN: [Capability; 1] = [Capability::ConfigurePlatform];

fn scoped_policy() -> Value {
    let unit = u64::from(u8::MAX);
    json!({"owners": [{"owner": "budget-test-owner", "policy": {
        "owner_day_limit": unit * unit, "turn_limit": unit,
        "input_token_units": 1, "output_token_units": 1,
        "max_unobserved_units": 1, "usage_timeout_seconds": 60
    }}]})
}

#[tokio::test]
async fn model_budget_settings_require_admin_cas_and_keep_values_out_of_audit() {
    let fixture = fixture(false);
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
    let fixture = fixture(false);
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
        json!({"owners": []})
    );
}

#[tokio::test]
async fn malformed_budget_policies_are_confidential_and_never_saved() {
    let fixture = fixture(false);
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
