use super::*;

const PATH: &str = "/api/v1/bridge-creation-target";

#[tokio::test]
async fn preview_has_no_effect_and_create_reuses_exact_identity_and_callback() {
    for organization in [false, true] {
        let (router, store, ports) = fixture();
        let path = if organization {
            format!("{PATH}?organization=org")
        } else {
            PATH.into()
        };
        let response = router
            .clone()
            .oneshot(request(
                Method::GET,
                &path,
                "owner",
                Some("create-one"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
        let preview: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 32768).await.unwrap()).unwrap();
        assert_eq!(preview.as_object().unwrap().len(), 3);
        assert_eq!(preview["owner"], create_body(organization)["owner"]);
        let id = preview["bridge_id"].as_str().unwrap();
        assert!(store.bridge(&BridgeId(id.into())).await.unwrap().is_none());
        assert_eq!(ports.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            send(
                &router,
                Method::GET,
                &path,
                "owner",
                Some("create-one"),
                None
            )
            .await
            .1,
            preview
        );
        let other = send(
            &router,
            Method::GET,
            &path,
            "owner",
            Some("another-create"),
            None,
        )
        .await;
        assert_eq!(other.0, StatusCode::OK);
        assert_ne!(other.1["bridge_id"], preview["bridge_id"]);
        let other_caller = send(
            &router,
            Method::GET,
            PATH,
            "another",
            Some("create-one"),
            None,
        )
        .await;
        assert_eq!(other_caller.0, StatusCode::OK);
        assert_ne!(other_caller.1["bridge_id"], preview["bridge_id"]);
        assert_eq!(create(&router, organization).await, id);
        let record = store.bridge(&BridgeId(id.into())).await.unwrap().unwrap();
        assert_eq!(
            record.current().unwrap().config.connector.callback_url,
            preview["callback_url"]
        );
        assert_eq!(record.revision, 1);
        assert_eq!(ports.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn preview_requires_current_management_and_does_not_authorize_later_creation() {
    let (router, store, ports) = fixture();
    let path = format!("{PATH}?organization=org");
    let (status, preview) = send(
        &router,
        Method::GET,
        &path,
        "owner",
        Some("create-one"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for (role, status) in [
        (1, StatusCode::FORBIDDEN),
        (0, StatusCode::NOT_FOUND),
        (3, StatusCode::SERVICE_UNAVAILABLE),
    ] {
        ports.role.store(role, Ordering::SeqCst);
        assert_eq!(
            send(
                &router,
                Method::GET,
                &path,
                "owner",
                Some("create-one"),
                None
            )
            .await
            .0,
            status
        );
        assert_eq!(
            send(
                &router,
                Method::POST,
                "/api/v1/bridges",
                "owner",
                Some("create-one"),
                Some(create_body(true))
            )
            .await
            .0,
            status
        );
    }
    ports.role.store(2, Ordering::SeqCst);
    ports.management.store(false, Ordering::SeqCst);
    for path in [PATH, &path] {
        let (status, body) = send(
            &router,
            Method::GET,
            path,
            "owner",
            Some("create-one"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["code"], "bridge_unavailable");
    }
    assert_eq!(
        send(
            &router,
            Method::POST,
            "/api/v1/bridges",
            "owner",
            Some("create-one"),
            Some(create_body(true))
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(store
        .bridge(&BridgeId(preview["bridge_id"].as_str().unwrap().into()))
        .await
        .unwrap()
        .is_none());
    assert_eq!(ports.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn preview_requires_auth_key_valid_query_and_configured_setup() {
    let (router, _, ports) = fixture();
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(PATH)
                .header("Idempotency-Key", "create-one")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    for (path, key, code) in [
        (PATH, None, "missing_idempotency_key"),
        (PATH, Some("bad key"), "invalid_idempotency_key"),
        (
            "/api/v1/bridge-creation-target?callback_url=https://attacker.example",
            Some("create-one"),
            "invalid_query",
        ),
    ] {
        let (status, body) = send(&router, Method::GET, path, "owner", key, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["code"], code);
    }
    let (status, body) = send(
        &super::super::bridge_router(None),
        Method::GET,
        PATH,
        "owner",
        Some("create-one"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "bridge_setup_unavailable");
    assert_eq!(ports.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn preview_openapi_requires_creation_key_and_exact_safe_response() {
    let doc = serde_json::to_value(super::super::openapi::document()).unwrap();
    let operation = &doc["paths"][PATH]["get"];
    let params = operation["parameters"].as_array().unwrap();
    assert!(params
        .iter()
        .any(|p| p["in"] == "header" && p["name"] == "Idempotency-Key" && p["required"] == true));
    assert!(params
        .iter()
        .any(|p| p["in"] == "query" && p["name"] == "organization" && p["required"] == false));
    let schema = &doc["components"]["schemas"]["BridgeCreationTargetShape"];
    assert_eq!(schema["properties"].as_object().unwrap().len(), 3);
    for field in ["bridge_id", "owner", "callback_url"] {
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!(field)));
    }
}
