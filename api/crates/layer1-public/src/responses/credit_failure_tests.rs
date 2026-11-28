use axum::{
    body::to_bytes,
    http::{Method, StatusCode},
};
use layer2_core::sandbox::{AgentLogKind, AgentStatus, TypedFailure};
use serde_json::Value;

use crate::rest::agents_tests::{
    conversation_fixture, insert_agent, json_response, request, response, seeded_agent,
};

#[tokio::test]
async fn insufficient_credit_terminal_is_consistent_on_retrieval_and_both_streams() {
    let (state, store) = conversation_fixture().await;
    let agent = seeded_agent("agt_credit", AgentStatus::Ready, 6, None);
    let detail = "The model request failed with HTTP 402 because the model billing account has insufficient credits. Earlier files, tool results, and partial output remain available. Ask the Sandbox owner to top up the account used for model requests, then send a new message to continue. This failed turn will not be replayed automatically.";
    insert_agent(
        &store,
        agent.clone(),
        vec![
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 3,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "Begin".into(),
            },
            AgentLogKind::AgentMessage {
                content: "Previous effects remain recorded.".into(),
            },
            AgentLogKind::TurnFailed {
                turn_sequence: 3,
                failure: TypedFailure {
                    class: "insufficient_credits".into(),
                    detail: detail.into(),
                },
            },
        ],
    )
    .await;
    let router = super::router(state.clone()).merge(crate::agui::router(state.clone()));
    let (status, retrieved) = json_response(
        &router,
        request(
            Method::GET,
            "/api/v1/responses/resp_agt_credit_3",
            "owner-a",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(retrieved["status"], "failed");
    assert_eq!(retrieved["error"]["code"], "insufficient_credits");
    assert_eq!(retrieved["error"]["message"], detail);

    let caller = layer2_core::Caller::new(
        layer2_core::SubjectId("owner-a".into()),
        layer2_core::Authorization::new([layer2_core::Capability::UseOwnSandboxes]),
    );
    let stream = super::stream::open(state, caller, agent, 3, None);
    let bytes = to_bytes(stream.into_body(), 1024 * 1024).await.unwrap();
    let events = frames(&bytes);
    assert_eq!(events.last().unwrap()["type"], "response.failed");
    assert_eq!(events.last().unwrap()["response"], retrieved);

    let (status, bytes) = response(
        &router,
        request(
            Method::GET,
            "/api/v1/agui/events?thread_id=agt_credit&follow=false",
            "owner-a",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let events = frames(&bytes);
    let errors: Vec<_> = events.iter().filter(|e| e["type"] == "RUN_ERROR").collect();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["code"], "insufficient_credits");
    assert_eq!(errors[0]["message"], detail);
    assert!(!events.iter().any(|e| e["type"] == "RUN_FINISHED"));
}

fn frames(bytes: &[u8]) -> Vec<Value> {
    std::str::from_utf8(bytes)
        .unwrap()
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
