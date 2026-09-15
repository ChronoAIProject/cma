use axum::http::{Method, StatusCode};
use layer2_core::{
    ids::AgentCommandId,
    sandbox::{
        agent_io::*,
        store::{AgentStore, PageRequest},
    },
    AgentId,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::super::controls::fixtures::{call, fixture, request, SOURCE};

const PATH: &str = "/api/v2/agents/agt_fixture/responses/resp_control_source/queued-cancellation";
fn body(revision: u64) -> Value {
    json!({"source_command_id": SOURCE, "expected_command_revision": revision})
}

#[tokio::test]
async fn queued_cancellation_http_recovers_receipt_and_emits_terminal_replay_without_a_turn() {
    let f = fixture(false).await;
    let before = f
        .store
        .get_agent(&AgentId("agt_fixture".into()))
        .await
        .unwrap();
    let (_, observed) = call(&f.router, Method::GET, PATH, "owner-a", None, None).await;
    assert_eq!(observed["cancellable"], true);
    assert_eq!(observed["response"]["turn_sequence"], Value::Null);
    let intent = body(observed["command_revision"].as_u64().unwrap());
    let (status, accepted) = call(
        &f.router,
        Method::POST,
        PATH,
        "owner-a",
        Some("cancel-original"),
        Some(intent.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(accepted["cancellation"]["status"], "completed");
    assert_eq!(accepted["response"]["status"], "cancelled");
    for field in ["turn_sequence", "delivery", "observed_cursor", "failure"] {
        assert_eq!(accepted["cancellation"][field], Value::Null);
        assert_eq!(accepted["response"][field], Value::Null);
    }
    let (_, recovered) = call(
        &f.router,
        Method::POST,
        PATH,
        "owner-a",
        Some("cancel-original"),
        Some(intent),
    )
    .await;
    assert_eq!(recovered, accepted);
    assert_eq!(
        f.store
            .get_agent(&AgentId("agt_fixture".into()))
            .await
            .unwrap(),
        before
    );
    let (_, observed) = call(&f.router, Method::GET, PATH, "owner-a", None, None).await;
    assert_eq!(observed["cancellable"], false);
    let (_, response) = call(
        &f.router,
        Method::GET,
        "/api/v2/responses/resp_control_source",
        "owner-a",
        None,
        None,
    )
    .await;
    assert_eq!(response["status"], "incomplete");
    assert_eq!(
        response["incomplete_details"],
        json!({"reason":"interrupted"})
    );
    assert_eq!(response["output"], json!([]));
    let events = f
        .router
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v2/responses/resp_control_source/events",
            "owner-a",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(events.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(events.into_body(), 64 * 1024)
        .await
        .unwrap();
    let stream = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(stream.contains("event: response.created"));
    assert!(stream.contains("event: response.incomplete"));
    assert!(!stream.contains("response.in_progress"));
    assert!(!stream.contains("output_item"));
    let tail = f
        .router
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v2/responses/resp_control_source/events?starting_after=0",
            "owner-a",
            None,
            None,
        ))
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(tail.into_body(), 64 * 1024)
        .await
        .unwrap();
    let stream = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(!stream.contains("event: response.created"));
    assert!(stream.contains("event: response.incomplete"));
    assert_eq!(
        f.store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        0
    );
}

#[tokio::test]
async fn queued_cancellation_http_distinguishes_exact_recovery_stale_proof_and_dispatch_conflict() {
    let f = fixture(false).await;
    let intent = body(1);
    let (_, accepted) = call(
        &f.router,
        Method::POST,
        PATH,
        "owner-a",
        Some("original"),
        Some(intent.clone()),
    )
    .await;
    for changed in [
        body(2),
        json!({"source_command_id":"cmd_other","expected_command_revision":1}),
    ] {
        let (status, problem) = call(
            &f.router,
            Method::POST,
            PATH,
            "owner-a",
            Some("original"),
            Some(changed),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(problem["error"]["code"], "agent_command_key_conflict");
    }
    let (status, problem) = call(
        &f.router,
        Method::POST,
        PATH,
        "owner-a",
        Some("another"),
        Some(intent),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        problem["error"]["code"],
        "queued_response_cancellation_not_admitted"
    );
    let command_id = accepted["cancellation"]["receipt"]["command_id"]
        .as_str()
        .unwrap();
    let (_, status) = call(
        &f.router,
        Method::GET,
        &format!("/api/v2/commands/{command_id}"),
        "owner-a",
        None,
        None,
    )
    .await;
    assert_eq!(status, accepted["cancellation"]);
    let bound = fixture(true).await;
    let (_, observed) = call(&bound.router, Method::GET, PATH, "owner-a", None, None).await;
    assert_eq!(observed["cancellable"], false);
    let (status, problem) = call(
        &bound.router,
        Method::POST,
        PATH,
        "owner-a",
        Some("already-dispatched"),
        Some(body(observed["command_revision"].as_u64().unwrap())),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        problem["error"]["code"],
        "queued_response_cancellation_conflict"
    );
}

#[tokio::test]
async fn queued_cancellation_http_requires_current_owner_and_strict_original_body() {
    let f = fixture(false).await;
    for method in [Method::GET, Method::POST] {
        let (status, _) = call(
            &f.router,
            method.clone(),
            PATH,
            "foreign",
            Some("foreign"),
            (method == Method::POST).then(|| body(1)),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    for value in [
        body(0),
        json!({"source_command_id":SOURCE,"expected_command_revision":1,"turn_sequence":4}),
        json!({"source_command_id":SOURCE,"expected_command_revision":1,"actor":"forged"}),
    ] {
        let (status, problem) = call(
            &f.router,
            Method::POST,
            PATH,
            "owner-a",
            Some("bad"),
            Some(value),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            problem["error"]["code"],
            "invalid_queued_cancellation_request"
        );
    }
    assert_eq!(
        f.store
            .get_command(&AgentCommandId(SOURCE.into()))
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentResponseStatus::Queued
    );
}
