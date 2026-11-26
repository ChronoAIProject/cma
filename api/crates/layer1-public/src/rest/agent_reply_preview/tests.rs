use super::*;
use crate::rest::agents_tests as fixture;
use axum::{
    body::to_bytes,
    http::{Method, StatusCode},
};
use layer2_core::sandbox::{
    store::{AgentLogStore, AgentStore},
    AgentStatus,
};
use serde_json::{json, Value};
use tower::ServiceExt;

#[tokio::test]
async fn ready_none_and_unavailable_are_distinct_private_wire_shapes() {
    for (result, expected) in [
        (
            AgentReplyPreview::Ready {
                source_through_seq: 9,
                reply: AssistantReplyPreview {
                    text: "Real reply".into(),
                    truncated: false,
                    message_sequence: 7,
                    turn_sequence: 3,
                    appended_at: "2026-11-20T00:00:00Z".into(),
                },
            },
            json!({"status":"ready","agent_id":"agt_preview","source_through_seq":9,"latest_assistant_reply":{"text":"Real reply","truncated":false,"message_sequence":7,"turn_sequence":3,"appended_at":"2026-11-20T00:00:00Z"}}),
        ),
        (
            AgentReplyPreview::None {
                source_through_seq: 2,
            },
            json!({"status":"none","agent_id":"agt_preview","source_through_seq":2,"latest_assistant_reply":null}),
        ),
        (
            AgentReplyPreview::Unavailable,
            json!({"status":"unavailable","agent_id":"agt_preview","source_through_seq":null,"latest_assistant_reply":null}),
        ),
    ] {
        let response = response("agt_preview".into(), result);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body, expected);
    }
}

#[tokio::test]
async fn unsupported_store_returns_unavailable_without_changing_the_log() {
    let fixture = fixture::fixture().await;
    let agent = fixture::seeded_agent("agt_preview", AgentStatus::Ready, 2, None);
    fixture::insert_agent(&fixture.store, agent.clone(), vec![]).await;
    let before = fixture.store.get_agent(&agent.id).await.unwrap();
    let log = fixture.store.agent_log(&agent.id, 0, 200).await.unwrap();
    let path = "/api/v1/agents/agt_preview/reply-preview";
    let response = fixture
        .router
        .clone()
        .oneshot(fixture::request(Method::GET, path, "owner-a", None, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["status"], "unavailable");
    assert_eq!(fixture.store.get_agent(&agent.id).await.unwrap(), before);
    assert_eq!(
        fixture.store.agent_log(&agent.id, 0, 200).await.unwrap(),
        log
    );
    let (status, _) = fixture::json_response(
        &fixture.router,
        fixture::request(Method::GET, path, "owner-b", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn shared_history_is_eligible_but_mismatched_callers_are_not() {
    use crate::rest::{resource_access_tests::context, shared_agent_tests};
    let (fixture, _, _, _) = shared_agent_tests::fixture(shared_agent_tests::Reply::Member).await;
    let path = "/api/v1/agents/agt_shared_read/reply-preview";
    let mut request = fixture::request(Method::GET, path, "recipient", None, None);
    request.extensions_mut().insert(context("recipient"));
    request
        .extensions_mut()
        .insert(crate::DelegatedCredential::new("request-credential".into()));
    assert_eq!(
        fixture::json_response(&fixture.router, request).await.0,
        StatusCode::OK
    );
    let caller = context("owner-a").caller;
    assert!(fixture
        .reads()
        .reply_preview(
            &caller,
            Some(&context("recipient")),
            None,
            &AgentId("agt_shared_read".into())
        )
        .await
        .is_err());
}
