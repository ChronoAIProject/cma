use super::*;

#[tokio::test]
async fn budget_save_before_reader_gate_refuses_disabled_and_enabled_policies() {
    let fixture = budget_fixture(false);
    let initial = response_json(send(&fixture.app, Method::GET, PATH, None, ADMIN).await).await;
    assert_eq!(initial["model_budget_writes_available"], false);
    let unauthorized = send(
        &fixture.app,
        Method::PATCH,
        PATH,
        Some(json!({"expected_revision":0,"model_budget":scoped_policy()})),
        [],
    )
    .await;
    assert_eq!(unauthorized.status(), StatusCode::FORBIDDEN);
    let spoofed_gate = send(
        &fixture.app,
        Method::PATCH,
        PATH,
        Some(
            json!({"expected_revision":0,"model_budget_writes_available":true,
            "model_budget":scoped_policy()}),
        ),
        ADMIN,
    )
    .await;
    assert_eq!(spoofed_gate.status(), StatusCode::UNPROCESSABLE_ENTITY);
    for enabled in [false, true] {
        let mut policy = scoped_policy();
        policy["enabled"] = json!(enabled);
        let mut settings = initial["settings"].clone();
        settings["model_budget"] = policy.clone();
        for (method, body) in [
            (
                Method::PATCH,
                json!({"expected_revision":0,"model_budget":policy}),
            ),
            (
                Method::PUT,
                json!({"expected_revision":0,"settings":settings}),
            ),
        ] {
            let refused = send(&fixture.app, method, PATH, Some(body), ADMIN).await;
            assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
            let body = response_json(refused).await;
            assert_eq!(body["code"], "model_budget_writes_unavailable");
            assert!(!body.to_string().contains("budget-test-owner"));
        }
    }
    let current = response_json(send(&fixture.app, Method::GET, PATH, None, ADMIN).await).await;
    assert_eq!(current["revision"], 0);
    assert_eq!(current["settings"], initial["settings"]);
    assert!(!fixture
        .activity
        .records
        .lock()
        .unwrap()
        .iter()
        .any(|r| r.action == ActivityAction::Configure));
    let unrelated = send(
        &fixture.app,
        Method::PATCH,
        PATH,
        Some(json!({"expected_revision":0,"marketplace":{"enabled":false}})),
        ADMIN,
    )
    .await;
    assert_eq!(unrelated.status(), StatusCode::OK);
}

#[tokio::test]
async fn compatible_reader_can_read_saved_disabled_values_after_write_gate_closes() {
    let store = Arc::new(MemorySettings::default());
    let writer = budget_fixture_with_store(true, store.clone());
    let mut policy = scoped_policy();
    policy["enabled"] = json!(false);
    let saved = send(
        &writer.app,
        Method::PATCH,
        PATH,
        Some(json!({"expected_revision":0,"model_budget":policy})),
        ADMIN,
    )
    .await;
    assert_eq!(saved.status(), StatusCode::OK);
    let saved = response_json(saved).await;
    assert_eq!(saved["model_budget_writes_available"], true);
    let reader = budget_fixture_with_store(false, store);
    let current = response_json(send(&reader.app, Method::GET, PATH, None, ADMIN).await).await;
    assert_eq!(current["model_budget_writes_available"], false);
    assert_eq!(current["revision"], saved["revision"]);
    assert_eq!(current["settings"]["model_budget"], policy);
    let refused = send(
        &reader.app,
        Method::PATCH,
        PATH,
        Some(json!({"expected_revision":1,"model_budget":policy})),
        ADMIN,
    )
    .await;
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    let current = response_json(send(&reader.app, Method::GET, PATH, None, ADMIN).await).await;
    assert_eq!(current["revision"], 1);
    assert_eq!(current["settings"]["model_budget"], policy);
}
