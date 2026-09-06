use axum::http::{Method, StatusCode};
use layer2_core::trigger_domain::{
    BrokerHandoffState, ServiceConnectionMetadata, ServiceConnectionMode, ServiceRequirementState,
};
use serde_json::json;

use super::support_tests::*;

#[tokio::test]
async fn connection_handoff_creation_replays_and_polling_persists_terminal_state() {
    let app = TestApp::new().await;
    app.broker.set_requirement(ServiceRequirementState::Missing);
    let trigger = create_trigger(&app, "always_new", "create-connection-trigger").await;
    let published = publish_trigger(
        &app,
        trigger["id"].as_str().unwrap(),
        "publish-connection-trigger",
    )
    .await;
    let (_, launch) = json_response(
        &app.router,
        visitor_request(
            Method::POST,
            &format!(
                "/api/v1/triggers/public/{}/launches",
                published["public_id"].as_str().unwrap()
            ),
            Some(launch_body(false)),
            Some("connection-launch"),
        ),
    )
    .await;
    let launch_id = launch["id"].as_str().unwrap();
    let request_body = json!({
        "service_id": "github",
        "launch_id": launch_id,
        "intent_id": "intent-one"
    });
    let owner = || Some((VISITOR, layer2_core::Authorization::none()));
    let first = raw_response(
        &app.router,
        request(
            Method::POST,
            "/api/v1/service-connections",
            Some(request_body.clone()),
            owner(),
            Some("connection-create"),
        ),
    )
    .await;
    assert_eq!(first.0, StatusCode::CREATED);
    let created: serde_json::Value = serde_json::from_slice(&first.2).unwrap();
    assert_eq!(
        created["hosted_url"],
        "https://identity.example/connect/intent-one"
    );
    assert!(created.get("external_id").is_none());
    let handoff_id = created["handoff_id"].as_str().unwrap();
    assert_eq!(app.broker.create_calls(), 1);

    let replay = raw_response(
        &app.router,
        request(
            Method::POST,
            "/api/v1/service-connections",
            Some(request_body),
            owner(),
            Some("connection-create"),
        ),
    )
    .await;
    assert_eq!(replay.0, first.0);
    assert_eq!(replay.1, first.1);
    assert_eq!(replay.2, first.2);
    assert_eq!(app.broker.create_calls(), 1);

    let (status, pending) = json_response(
        &app.router,
        request(
            Method::GET,
            &format!("/api/v1/service-connections/{handoff_id}"),
            None,
            owner(),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(pending["state"], "pending");
    app.broker.observe(BrokerHandoffState::Satisfied);
    let (status, satisfied) = json_response(
        &app.router,
        request(
            Method::GET,
            &format!("/api/v1/service-connections/{handoff_id}"),
            None,
            owner(),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{satisfied}");
    assert_eq!(satisfied["state"], "satisfied");

    let (status, hidden) = json_response(
        &app.router,
        request(
            Method::GET,
            &format!("/api/v1/service-connections/{handoff_id}"),
            None,
            Some((OTHER, visitor_authority())),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(hidden["code"], "service_connection_not_found");
}

#[tokio::test]
async fn missing_or_foreign_launch_is_hidden_before_connection_command_reservation() {
    let app = TestApp::new().await;
    let before = app.operation_reservations();
    let (status, problem) = json_response(
        &app.router,
        visitor_request(
            Method::POST,
            "/api/v1/service-connections",
            Some(json!({
                "service_id": "github",
                "launch_id": "lch_missing",
                "intent_id": "intent-missing"
            })),
            Some("missing-connection"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(problem["code"], "service_connection_not_found");
    assert_eq!(app.operation_reservations(), before);
}

#[tokio::test]
async fn requirement_metadata_is_owner_only_and_does_not_rewrite_launch_projection() {
    let app = TestApp::new().await;
    let trigger = create_trigger(&app, "always_new", "metadata-create-trigger").await;
    let published = publish_trigger(
        &app,
        trigger["id"].as_str().unwrap(),
        "metadata-publish-trigger",
    )
    .await;
    let (_, launch) = json_response(
        &app.router,
        visitor_request(
            Method::POST,
            &format!(
                "/api/v1/triggers/public/{}/launches",
                published["public_id"].as_str().unwrap()
            ),
            Some(launch_body(false)),
            Some("metadata-launch"),
        ),
    )
    .await;
    let id = launch["id"].as_str().unwrap();
    app.broker.set_metadata(vec![ServiceConnectionMetadata {
        service: layer2_core::ServiceId::new("github"),
        display_name: Some("GitHub".into()),
        connection_mode: ServiceConnectionMode::Manual,
        setup_url: Some("https://nyx.example/services".into()),
    }]);
    let requirement_path = format!("/api/v1/launches/{id}/requirements");
    for (path, caller) in [
        (&requirement_path, OTHER),
        (
            &"/api/v1/launches/lch_absent/requirements".to_owned(),
            VISITOR,
        ),
    ] {
        let (status, _) = json_response(
            &app.router,
            request(
                Method::GET,
                path,
                None,
                Some((caller, visitor_authority())),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    assert_eq!(app.broker.metadata_reads(), 0);
    let (status, requirements) = json_response(
        &app.router,
        visitor_request(Method::GET, &requirement_path, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{requirements}");
    assert_eq!(
        requirements["services"],
        json!([{"service":"github","state":"missing","display_name":"GitHub","connection_mode":"manual","setup_url":"https://nyx.example/services"}])
    );
    assert_eq!(
        requirements["checked_at"],
        launch["requirements"]["checked_at"]
    );
    let (status, after) = json_response(
        &app.router,
        visitor_request(Method::GET, &format!("/api/v1/launches/{id}"), None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(after, launch);
    assert!(after["requirements"]["services"][0]
        .get("connection_mode")
        .is_none());
}
