use axum::http::{Method, StatusCode};
use layer2_core::trigger_domain::{LaunchCommit, LaunchFirstMessageCommand, LaunchStore};
use layer2_core::{
    ids::{AgentCommandId, AgentResponseId},
    AgentId, LaunchId, SubjectId,
};
use serde_json::json;

use super::support_tests::*;

#[tokio::test]
async fn launch_association_uses_owned_reads_and_legacy_omission_without_command_mutation() {
    let app = TestApp::new().await;
    let trigger = create_trigger(&app, "always_new", "association-trigger").await;
    let published =
        publish_trigger(&app, trigger["id"].as_str().unwrap(), "association-publish").await;
    let (status, legacy) = json_response(
        &app.router,
        visitor_request(
            Method::POST,
            &format!(
                "/api/v1/triggers/public/{}/launches",
                published["public_id"].as_str().unwrap()
            ),
            Some(launch_body(false)),
            Some("association-launch"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(legacy.get("first_message_command").is_none());
    let id = LaunchId(legacy["id"].as_str().unwrap().into());
    let mut launch = app
        .launches
        .launch(&id, &SubjectId::new(VISITOR))
        .await
        .unwrap()
        .unwrap();
    let revision = launch.revision;
    // This transport fixture supplies a trusted store projection. Separate real
    // Mongo fixtures prove its immutable binding, snapshot and owner checks.
    launch.first_message_command = Some(LaunchFirstMessageCommand {
        agent_id: AgentId("agt_original".into()),
        command_id: AgentCommandId("cmd_original".into()),
        response_id: AgentResponseId("rsp_original".into()),
    });
    launch.revision += 1;
    app.launches
        .commit_launch(LaunchCommit {
            expected_revision: revision,
            event_at: launch.updated_at.clone(),
            launch,
            events: vec![],
        })
        .await
        .unwrap();
    let reserved = app.operation_reservations();
    let path = format!("/api/v1/launches/{id}");
    let expected =
        json!({"agent_id":"agt_original","command_id":"cmd_original","response_id":"rsp_original"});
    let (status, own) =
        json_response(&app.router, visitor_request(Method::GET, &path, None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(own["first_message_command"], expected);
    for private in [
        "input_protocol",
        "input_binding_sha256",
        "initiation",
        "owner",
    ] {
        assert!(own.get(private).is_none());
    }
    let (status, page) = json_response(
        &app.router,
        visitor_request(Method::GET, "/api/v1/launches", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"][0]["first_message_command"], expected);
    let (status, denied) = json_response(
        &app.router,
        publisher_request(Method::GET, &path, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(denied.get("first_message_command").is_none());
    let (status, _) =
        json_response(&app.router, request(Method::GET, &path, None, None, None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(app.operation_reservations(), reserved);
}
