use super::*;

fn request_policy() -> Value {
    let mut policy = scoped_policy();
    policy["request_models"] = json!([{"model":"approved", "input_overhead_tokens": 1}]);
    policy
}

#[tokio::test]
async fn foundation_write_gate_does_not_authorize_new_request_policy_shapes() {
    let fixture = budget_fixture(true);
    let initial = response_json(send(&fixture.app, Method::GET, PATH, None, ADMIN).await).await;
    assert_eq!(initial["model_budget_writes_available"], true);
    assert_eq!(initial["model_request_budget_writes_available"], false);
    for enabled in [false, true] {
        let mut policy = request_policy();
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
            assert_eq!(
                response_json(refused).await["code"],
                "model_budget_writes_unavailable"
            );
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
        .any(|record| record.action == ActivityAction::Configure));
    let foundation = send(
        &fixture.app,
        Method::PATCH,
        PATH,
        Some(json!({"expected_revision":0,"model_budget":scoped_policy()})),
        ADMIN,
    )
    .await;
    assert_eq!(foundation.status(), StatusCode::OK);
}

#[tokio::test]
async fn request_policy_writer_requires_both_gates_and_reader_survives_closure() {
    let store = Arc::new(MemorySettings::default());
    let closed = budget_fixture_with_gates(false, true, store.clone());
    let read = response_json(send(&closed.app, Method::GET, PATH, None, ADMIN).await).await;
    assert_eq!(read["model_request_budget_writes_available"], false);
    let mut policy = request_policy();
    policy["enabled"] = json!(false);
    let body = json!({"expected_revision":0,"model_budget":policy});
    assert_eq!(
        send(&closed.app, Method::PATCH, PATH, Some(body.clone()), ADMIN)
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let writer = budget_fixture_with_gates(true, true, store.clone());
    assert_eq!(
        send(&writer.app, Method::PATCH, PATH, Some(body.clone()), [])
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let mut spoofed = body.clone();
    spoofed["model_request_budget_writes_available"] = json!(true);
    assert_eq!(
        send(&writer.app, Method::PATCH, PATH, Some(spoofed), ADMIN)
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let saved = send(&writer.app, Method::PATCH, PATH, Some(body.clone()), ADMIN).await;
    assert_eq!(saved.status(), StatusCode::OK);
    assert_eq!(
        response_json(saved).await["model_request_budget_writes_available"],
        true
    );
    assert_eq!(
        send(&writer.app, Method::PATCH, PATH, Some(body), ADMIN)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let reader = budget_fixture_with_gates(true, false, store);
    let read = response_json(send(&reader.app, Method::GET, PATH, None, ADMIN).await).await;
    assert_eq!(read["settings"]["model_budget"], policy);
    assert_eq!(read["revision"], 1);
    assert_eq!(read["model_request_budget_writes_available"], false);
    assert_eq!(
        send(
            &reader.app,
            Method::PATCH,
            PATH,
            Some(json!({"expected_revision":1,"model_budget":policy})),
            ADMIN
        )
        .await
        .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let read = response_json(send(&reader.app, Method::GET, PATH, None, ADMIN).await).await;
    assert_eq!(read["revision"], 1);
    assert_eq!(read["settings"]["model_budget"], policy);
}
