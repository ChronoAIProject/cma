use axum::http::{Method, StatusCode};
use layer2_core::sandbox::{AgentLogKind, AgentStatus};
use serde_json::{json, Value};

use crate::rest::agents_tests::{
    conversation_fixture, insert_agent, json_response, request, response, seeded_agent,
};

#[tokio::test]
async fn unknown_threads_are_refused_without_creating_agents() {
    let (state, store) = conversation_fixture().await;
    let router = super::router(state);
    let (status, body) = json_response(&router, request(Method::POST, "/api/v1/agui/run", "owner-a", None,
        Some(json!({"threadId": "agt_unknown", "runId": "client-run", "messages": [{"role": "user", "content": "Hello"}]})))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "thread_not_found");
    use layer2_core::sandbox::store::AgentStore;
    assert!(store
        .list_agents(
            &layer2_core::SubjectId("owner-a".into()),
            &Default::default()
        )
        .await
        .unwrap()
        .items
        .is_empty());
}

#[tokio::test]
async fn replay_uses_agent_thread_turn_sequence_and_only_final_frame_cursors() {
    let (state, store) = conversation_fixture().await;
    let agent = seeded_agent("agt_replay", AgentStatus::Ready, 6, None);
    insert_agent(
        &store,
        agent,
        vec![
            AgentLogKind::TurnStarted {
                turn_sequence: 3,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "Question".into(),
            },
            AgentLogKind::AgentMessage {
                content: "Answer".into(),
            },
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
        ],
    )
    .await;
    let router = super::router(state);
    let (status, body) = response(
        &router,
        request(
            Method::GET,
            "/api/v1/agui/events?thread_id=agt_replay&follow=false",
            "owner-a",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = std::str::from_utf8(&body).unwrap();
    let frames = text
        .split("\n\n")
        .filter(|frame| frame.contains("data:"))
        .collect::<Vec<_>>();
    assert_eq!(frames.len(), 8);
    let values = frames
        .iter()
        .map(|frame| {
            serde_json::from_str::<Value>(
                frame
                    .lines()
                    .find_map(|line| line.strip_prefix("data: "))
                    .unwrap(),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    assert!(values
        .iter()
        .all(|value| value["threadId"] == "agt_replay" && value["runId"] == "3"));
    assert_eq!(values[0]["type"], "RUN_STARTED");
    assert_eq!(values[7]["type"], "RUN_FINISHED");
    assert!(!frames[1].contains("id:"));
    assert!(!frames[2].contains("id:"));
    assert!(frames[3].contains("id: 4"));
    assert!(!frames[4].contains("id:"));
    assert!(frames[6].contains("id: 5"));
    let (_, resumed) = response(
        &router,
        request(
            Method::GET,
            "/api/v1/agui/events?thread_id=agt_replay&starting_after=4&follow=false",
            "owner-a",
            None,
            None,
        ),
    )
    .await;
    let resumed = std::str::from_utf8(&resumed).unwrap();
    assert!(!resumed.contains("Question"));
    assert!(resumed.contains("Answer"));
    assert!(resumed.contains("\"runId\":\"3\""));
    let (status, body) = json_response(
        &router,
        request(
            Method::GET,
            "/api/v1/agui/events?thread_id=agt_replay&starting_after=7&follow=false",
            "owner-a",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "cursor_beyond_end");
    assert_eq!(body["head"], 6);
}

#[tokio::test]
async fn companion_is_owner_scoped_and_never_leaks_runtime_or_credential_authority() {
    let (state, store) = conversation_fixture().await;
    insert_agent(
        &store,
        seeded_agent("agt_context", AgentStatus::Ready, 2, None),
        vec![],
    )
    .await;
    let router = super::router(state);
    let path = "/api/v1/agui/agents/agt_context/sandbox";
    let (status, body) =
        json_response(&router, request(Method::GET, path, "owner-a", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["threadId"], "agt_context");
    assert_eq!(body["sandbox"]["id"], "sbx_route");
    assert_eq!(body["workspace"]["id"], "wks_route");
    assert_eq!(body["worktree"]["id"], "wkt_agt_context");
    assert!(body.get("firstMessage").is_some());
    assert!(!body.to_string().contains("runtime_id"));
    assert!(!body.to_string().contains("credential"));
    let foreign = response(&router, request(Method::GET, path, "owner-b", None, None)).await;
    let missing = response(
        &router,
        request(
            Method::GET,
            "/api/v1/agui/agents/agt_missing/sandbox",
            "owner-b",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(foreign, missing);
}

#[test]
fn capabilities_replace_retired_companions_with_read_only_sandbox_facts() {
    let document = super::capabilities::document();
    assert_eq!(
        document["features"]["custom"]["cma:sandbox"]["mutations"],
        false
    );
    assert!(document["features"]["custom"].get("cma:chat").is_none());
    assert!(document["features"]["custom"].get("cma:trigger").is_none());
}
