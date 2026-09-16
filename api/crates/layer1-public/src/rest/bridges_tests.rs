pub(super) mod fixture;
mod reuse;
use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
};
use fixture::*;
use layer2_core::bridge::{BridgeId, BridgeStore};
use serde_json::json;
use std::sync::atomic::Ordering;
use tower::ServiceExt;

#[tokio::test]
async fn owner_create_replay_configuration_and_revision_fences_use_server_selected_custody() {
    let (router, store, _) = fixture();
    let id = create(&router, false).await;
    let path = format!("/api/v1/bridges/{id}");
    let (status, replay) = send(
        &router,
        Method::POST,
        "/api/v1/bridges",
        "owner",
        Some("create-one"),
        Some(create_body(false)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(replay["bridge_id"], id);
    let record = store.bridge(&BridgeId(id.clone())).await.unwrap().unwrap();
    let custody = record.current().unwrap().config.connector.custody.clone();
    let response = router
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("{path}/configuration"),
            "owner",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    let bytes = to_bytes(response.into_body(), 32768).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        value["callback_url"],
        format!("https://cma.example/api/v1/bridge-callbacks/{id}")
    );
    assert_eq!(
        value["config"]["binding"]["route_ids"],
        json!(["route-a", "route-b"])
    );
    let wire = String::from_utf8(bytes.to_vec()).unwrap();
    for hidden in [custody.as_str(), "issuer", "base_url"] {
        assert!(!wire.contains(hidden))
    }
    let mut config = configuration();
    config["display_name"] = json!("Renamed");
    let update = json!({"expected_revision":1,"config":config,"previous_work":null});
    let first = send(
        &router,
        Method::PUT,
        &format!("{path}/configuration"),
        "owner",
        Some("rename"),
        Some(update.clone()),
    )
    .await;
    assert_eq!(first.0, StatusCode::OK);
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &format!("{path}/configuration"),
            "owner",
            Some("rename"),
            Some(update.clone())
        )
        .await,
        first
    );
    assert_eq!(
        send(
            &router,
            Method::PUT,
            &format!("{path}/configuration"),
            "owner",
            Some("another"),
            Some(update)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let current = store.bridge(&BridgeId(id)).await.unwrap().unwrap();
    assert_eq!(current.revision, 2);
    assert_eq!(current.current().unwrap().config.connector.custody, custody);
    assert_eq!(current.generations.len(), 2);
}

#[tokio::test]
async fn current_org_members_see_summary_but_never_setup_inventory_config_or_key_ports() {
    let (router, _, ports) = fixture();
    let id = create(&router, true).await;
    let path = format!("/api/v1/bridges/{id}");
    ports.role.store(1, Ordering::SeqCst);
    let (status, view) = send(&router, Method::GET, &path, "member", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["can_manage"], false);
    for field in [
        "config",
        "key_id",
        "route_ids",
        "credential",
        "callback_url",
    ] {
        assert!(view.get(field).is_none());
    }
    for (method, path, body) in [
        (Method::GET, format!("{path}/configuration"), None),
        (
            Method::GET,
            "/api/v1/bridge-connectors?organization=org".into(),
            None,
        ),
        (
            Method::PUT,
            format!("{path}/credential"),
            Some(json!({"expected_revision":1,"key":"fixture-selected-secret"})),
        ),
    ] {
        let (status, _) = send(&router, method, &path, "member", None, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    assert_eq!(ports.calls.load(Ordering::SeqCst), 0);
    ports.role.store(0, Ordering::SeqCst);
    assert_eq!(
        send(&router, Method::GET, &path, "member", None, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    ports.role.store(3, Ordering::SeqCst);
    assert_eq!(
        send(&router, Method::GET, &path, "member", None, None)
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn incompatible_placement_and_injected_endpoints_fail_before_creation() {
    let (router, _, _) = fixture();
    for injection in ["issuer", "base_url", "custody", "callback_url"] {
        let mut body = create_body(false);
        body["config"]["binding"][injection] = json!("https://attacker.example");
        assert_eq!(
            send(
                &router,
                Method::POST,
                "/api/v1/bridges",
                "owner",
                Some(injection),
                Some(body)
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let mut body = create_body(false);
    body["config"]["placement"] = json!({"kind":"existing_agent","target":{"sandbox_id":"sb","workspace_id":"ws","worktree_id":"wt","agent_id":"ag"}});
    assert_eq!(
        send(
            &router,
            Method::POST,
            "/api/v1/bridges",
            "owner",
            Some("misleading-isolation"),
            Some(body)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (status, page) = send(&router, Method::GET, "/api/v1/bridges", "owner", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"], json!([]));
}

#[tokio::test]
async fn key_material_is_write_only_and_unavailable_renewal_never_enables_or_retires_resources() {
    let (router, store, _) = fixture();
    let id = create(&router, false).await;
    let path = format!("/api/v1/bridges/{id}");
    let (status, value) = send(
        &router,
        Method::PUT,
        &format!("{path}/credential"),
        "owner",
        None,
        Some(json!({"expected_revision":1,"key":"fixture-selected-secret"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value, json!({"revision":1,"present":true,"revoked":false}));
    for (endpoint, body) in [
        ("validate", json!({"expected_revision":1})),
        (
            "commands",
            json!({"expected_revision":1,"command":{"kind":"enable"}}),
        ),
    ] {
        let (status, value) = send(
            &router,
            Method::POST,
            &format!("{path}/{endpoint}"),
            "owner",
            Some(endpoint),
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(value["code"], "bridge_renewable_authority_required");
    }
    assert_eq!(
        store
            .bridge(&BridgeId(id.clone()))
            .await
            .unwrap()
            .unwrap()
            .revision,
        1
    );
    let(status,_)=send(&router,Method::POST,&format!("{path}/commands"),"owner",Some("retire"),Some(json!({"expected_revision":1,"command":{"kind":"retire","accepted_work":"suspend_original"}}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        store
            .bridge(&BridgeId(id))
            .await
            .unwrap()
            .unwrap()
            .generations
            .len(),
        1
    );
}

#[tokio::test]
async fn missing_auth_and_unconfigured_provider_have_explicit_nonsecret_outcomes() {
    let router = super::bridge_router(None);
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/bridges")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let (status, value) = send(&router, Method::GET, "/api/v1/bridges", "owner", None, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(value["code"], "bridge_setup_unavailable");
}

#[test]
fn openapi_bridge_configuration_excludes_custody_and_marks_import_secret_write_only() {
    let doc = serde_json::to_value(super::openapi::document()).unwrap();
    let schemas = &doc["components"]["schemas"];
    assert!(doc["paths"]["/api/v1/bridge-connectors/capabilities"]["get"].is_object());
    assert!(schemas["BridgeCapabilityCatalogShape"]["properties"]["source_revision"].is_object());
    let props = schemas["BridgeBindingShape"]["properties"]
        .as_object()
        .unwrap();
    assert_eq!(props.len(), 4);
    assert!(!props.contains_key("custody"));
    assert_eq!(
        schemas["ImportBridgeKeyRequest"]["properties"]["key"]["writeOnly"],
        true
    );
    let old: super::bridges::shapes::ImportBridgeKeyRequest =
        serde_json::from_value(json!({"expected_revision":1,"key":"fixture-selected-secret"}))
            .unwrap();
    assert!(!old.retain_control_plane_authority);
    let explicit: super::bridges::shapes::ImportBridgeKeyRequest = serde_json::from_value(
        json!({"expected_revision":1,"key":"fixture-selected-secret","retain_control_plane_authority":true}),
    ).unwrap();
    assert!(explicit.retain_control_plane_authority);
    assert!(serde_json::from_value::<super::bridges::shapes::ImportBridgeKeyRequest>(
        json!({"expected_revision":1,"key":"fixture-selected-secret","retain_control_plane_authority":"true"}),
    ).is_err());
}

#[tokio::test]
async fn capabilities_require_current_owner_admin_and_old_validators_default_unavailable() {
    let (router, _, ports) = fixture();
    let response = router
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/bridge-connectors/capabilities",
            "owner",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    assert_eq!(
        response.headers()[header::X_CONTENT_TYPE_OPTIONS],
        "nosniff"
    );
    let wire: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 32768).await.unwrap()).unwrap();
    assert_eq!(
        wire,
        json!({"version":1,"source_revision":null,"connection_validation_available":false,"connectors":[],"placements":[]})
    );
    assert_eq!(ports.calls.load(Ordering::SeqCst), 0);
    ports.role.store(1, Ordering::SeqCst);
    let denied = send(
        &router,
        Method::GET,
        "/api/v1/bridge-connectors/capabilities?organization=org",
        "owner",
        None,
        None,
    )
    .await;
    assert_eq!(denied.0, StatusCode::FORBIDDEN);
    ports.role.store(2, Ordering::SeqCst);
    assert_eq!(
        send(
            &router,
            Method::GET,
            "/api/v1/bridge-connectors/capabilities?organization=org",
            "owner",
            None,
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    ports.role.store(0, Ordering::SeqCst);
    assert_eq!(
        send(
            &router,
            Method::GET,
            "/api/v1/bridge-connectors/capabilities?organization=org",
            "owner",
            None,
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(ports.calls.load(Ordering::SeqCst), 0);
}
