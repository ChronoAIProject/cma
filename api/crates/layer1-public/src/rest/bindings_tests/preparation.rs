use super::*;

fn inspection(input: &Value) -> Value {
    let now = layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339()).unwrap();
    json!({"schema_version":1,"admission_id":"ordinary-event","target_id":"consumer-one",
        "request_hash":"a".repeat(64),"owner":"owner","admitted_at":now-1,"expires_at":now+600,
        "agent_key":input["agent_key"]})
}

#[tokio::test]
async fn inspection_is_authenticated_read_only_and_never_exposes_credentials() {
    let (app, store) = app();
    let input = body();
    let path = "/api/v1/cmaeg/bindings/binding-one";
    assert_eq!(
        app.clone()
            .oneshot(request("PUT", path, Some(input.clone()), Some(authority())))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let original = store.binding("binding-one").await.unwrap();
    let inspect = format!("{path}/preparation-inspections");
    for (authority, status) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(Authorization::none()), StatusCode::FORBIDDEN),
        (
            Some(Authorization::new([Capability::ManageGatewayRecords])),
            StatusCode::FORBIDDEN,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(request(
                "POST",
                &inspect,
                Some(inspection(&input)),
                authority,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &inspect,
            Some(inspection(&input)),
            Some(authority()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await,
        json!({"state":"pinned","version":1,"binding_id":"binding-one","generation":1})
    );
    let mut foreign = inspection(&input);
    foreign["owner"] = "other-owner".into();
    assert_eq!(
        app.oneshot(request("POST", &inspect, Some(foreign), Some(authority())))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(store.binding("binding-one").await.unwrap(), original);
}

#[tokio::test]
async fn preparation_writers_default_off_and_enabled_requests_require_the_exact_unique_key() {
    let intent = BindingPreparationIntent {
        version: 1,
        admission_id: "ordinary-event".into(),
        target_id: "consumer-one".into(),
        request_hash: "a".repeat(64),
        binding_id: "binding-one".into(),
        owner: SubjectId::new("owner"),
        manager: SubjectId::new("manager"),
        generation: 1,
        binding_revision: 1,
        binding_digest: "b".repeat(64),
        sandbox_digest: "c".repeat(64),
        workspace_digest: "d".repeat(64),
        admitted_at: 1,
        expires_at: 600,
    };
    let body = json!({"schema_version":1,"intent":intent,"agent_key":"nyx_test_key"});
    let path = "/api/v1/cmaeg/bindings/binding-one/preparations";
    let (disabled, store) = app();
    assert_eq!(
        disabled
            .oneshot(request("POST", path, Some(body.clone()), Some(authority())))
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(store.binding("binding-one").await.unwrap().is_none());
    let (enabled, _) = app_config(None, None, true).unwrap();
    for keys in [
        vec![],
        vec!["wrong".to_owned()],
        vec![
            intent.operation_id().unwrap(),
            intent.operation_id().unwrap(),
        ],
    ] {
        let mut req = request("POST", path, Some(body.clone()), Some(authority()));
        for key in keys {
            req.headers_mut()
                .append("idempotency-key", key.parse().unwrap());
        }
        assert_eq!(
            enabled.clone().oneshot(req).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }
    let mut req = request("POST", path, Some(body), Some(authority()));
    req.headers_mut().insert(
        "idempotency-key",
        intent.operation_id().unwrap().parse().unwrap(),
    );
    // A valid operation key still requires the current binding authority.
    assert_eq!(
        enabled.oneshot(req).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn conversation_generation_is_enforced_for_both_agent_and_source_user_affinity() {
    for affinity in ["conversation", "source_user"] {
        let (app, store) = app();
        let mut input = body();
        input["profile"]["environment"]["session_affinity"] = affinity.into();
        input["profile_digest"] =
            format!("sha256:{}", content_digest(&input["profile"]).unwrap()).into();
        let path = "/api/v1/cmaeg/bindings/binding-one";
        assert_eq!(
            app.clone()
                .oneshot(request("PUT", path, Some(input), Some(authority())))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let mut binding = store.binding("binding-one").await.unwrap().unwrap();
        let revision = binding.revision;
        binding.state = BindingState::Ready;
        binding.work = BindingWork::Idle;
        binding
            .advance_revision(layer2_core::now_rfc3339())
            .unwrap();
        create_backing_resources(&store, &binding).await;
        store.save_binding(Some(revision), binding).await.unwrap();
        let path = format!("{path}/conversations/conversation_one");
        for (generation, status) in [("2", StatusCode::CONFLICT), ("0", StatusCode::BAD_REQUEST)] {
            let mut req = request("PUT", &path, None, Some(authority()));
            req.headers_mut()
                .insert("x-cma-binding-generation", generation.parse().unwrap());
            assert_eq!(app.clone().oneshot(req).await.unwrap().status(), status);
        }
        assert!(store
            .binding_conversations("binding-one", 1, None, 100)
            .await
            .unwrap()
            .is_empty());
        let mut req = request("PUT", &path, None, Some(authority()));
        req.headers_mut()
            .insert("x-cma-binding-generation", "1".parse().unwrap());
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(
            response.status(),
            if affinity == "source_user" {
                StatusCode::OK
            } else {
                StatusCode::CREATED
            }
        );
    }
}
