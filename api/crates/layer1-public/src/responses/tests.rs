use axum::{
    body::to_bytes,
    http::{Method, StatusCode},
};
use layer2_core::sandbox::{AgentLogKind, AgentStatus};
use serde_json::{json, Value};

use crate::rest::agents_tests::{
    conversation_fixture, insert_agent, json_response, request, response, seeded_agent,
};

#[tokio::test]
async fn a_stream_read_failure_does_not_fabricate_a_durable_response_failure() {
    let (state, _) = conversation_fixture().await;
    let agent = seeded_agent("agt_unreadable", AgentStatus::Working, 10, Some(3));
    let caller = layer2_core::Caller::new(
        layer2_core::SubjectId("owner-a".into()),
        layer2_core::Authorization::new([layer2_core::Capability::UseOwnSandboxes]),
    );
    let stream = super::stream::open(state, caller, agent, 3, None);
    let bytes = to_bytes(stream.into_body(), 65536).await.unwrap();
    assert!(bytes.is_empty());
}

#[tokio::test]
async fn conversations_create_in_default_context_and_return_persisted_agent_identity() {
    let (state, store) = conversation_fixture().await;
    let router = super::router(state);
    let (status, conversation) = json_response(
        &router,
        request(
            Method::POST,
            "/api/v1/conversations",
            "owner-a",
            None,
            Some(json!({})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(conversation["object"], "conversation");
    assert!(conversation["id"].as_str().unwrap().starts_with("agt_"));
    assert_eq!(conversation["metadata"]["sandbox"], "sbx_route");
    assert_eq!(conversation["metadata"]["workspace"], "wks_route");
    assert_eq!(conversation["metadata"]["agent_profile"], "agp_route");
    use layer2_core::sandbox::store::AgentStore;
    let stored = store
        .get_agent(&layer2_core::AgentId(
            conversation["id"].as_str().unwrap().into(),
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, AgentStatus::Starting);
    assert!(stored.conversation.first_message.is_none());
    let (_, retrieved) = json_response(
        &router,
        request(
            Method::GET,
            &format!("/api/v1/conversations/{}", stored.id),
            "owner-a",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(retrieved, conversation);
    let (status, body) = json_response(
        &router,
        request(
            Method::POST,
            "/api/v1/conversations",
            "owner-a",
            None,
            Some(json!({"metadata": {"workspace": "wks_route", "sandbox": "sbx_mismatch"}})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "invalid_context_override");
    let (status, body) = json_response(
        &router,
        request(
            Method::POST,
            "/api/v1/conversations",
            "owner-a",
            None,
            Some(json!({"metadata": {"instructions": "Do not accept"}})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["field"], "metadata");
}

#[tokio::test]
async fn launch_only_visitors_receive_sandbox_required_without_context_creation() {
    let (state, _) = conversation_fixture().await;
    let router = super::router(state);
    let mut request = request(
        Method::POST,
        "/api/v1/conversations",
        "visitor",
        None,
        Some(json!({})),
    );
    request
        .extensions_mut()
        .insert(layer2_core::Authorization::new([
            layer2_core::Capability::LaunchTriggers,
        ]));
    let (status, body) = json_response(&router, request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "sandbox_required");
}

#[tokio::test]
async fn official_text_request_forms_and_presence_aware_field_refusals_are_preserved() {
    for input in [
        json!("Hello"),
        json!([{"role": "user", "content": "Hello"}]),
        json!([{"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Hello"}]}]),
    ] {
        let parsed =
            super::parse::parse(json!({"conversation": "agt_one", "input": input, "stream": true}))
                .unwrap();
        assert_eq!(parsed.input, "Hello");
    }
    for field in [
        "instructions",
        "reasoning",
        "tools",
        "tool_choice",
        "background",
        "include",
        "stream_options",
    ] {
        let mut body = json!({"conversation": "agt_one", "input": "Hello", "stream": true});
        body[field] = Value::Null;
        let response = super::parse::parse(body).err().unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(body["code"], "responses_field_unsupported");
        assert_eq!(body["field"], field);
    }
    for unsupported in [
        json!({"conversation": "agt_one", "input": "Hello", "stream": false}),
        json!({"input": "Hello", "stream": true}),
        json!({"conversation": "agt_one", "input": [{"role": "system", "content": "No"}], "stream": true}),
        json!({"conversation": "agt_one", "input": "Hello", "stream": true, "store": false}),
    ] {
        assert!(super::parse::parse(unsupported).is_err());
    }
}

async fn completed() -> (crate::AgentConversationState, layer2_core::sandbox::Agent) {
    let (state, store) = conversation_fixture().await;
    let agent = seeded_agent("agt_response", AgentStatus::Ready, 10, None);
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
                content: "Question".into(),
            },
            AgentLogKind::TurnMetadataSet {
                turn_sequence: 3,
                metadata: [("project".into(), "proof".into())].into(),
            },
            AgentLogKind::AgentMessage {
                content: "First answer".into(),
            },
            AgentLogKind::ToolCallStarted {
                call_id: "call-proof".into(),
                title: "Read proof".into(),
                kind: "read".into(),
                status: "in_progress".into(),
            },
            AgentLogKind::AgentMessage {
                content: "Second answer".into(),
            },
            AgentLogKind::Usage {
                input_tokens: 10,
                output_tokens: 7,
                context_used: 17,
                context_window: Some(200_000),
            },
            AgentLogKind::UsageUpdated {
                input_tokens: 999,
                output_tokens: 500,
                context_used: 17,
                context_window: Some(200_000),
                turn_sequence: Some(3),
                turn_input_tokens: 10,
                turn_output_tokens: 7,
            },
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
        ],
    )
    .await;
    (state, agent)
}

#[tokio::test]
async fn stream_retrieve_and_items_project_the_same_durable_turn() {
    let (state, agent) = completed().await;
    let caller = layer2_core::Caller::new(
        layer2_core::SubjectId("owner-a".into()),
        layer2_core::Authorization::new([layer2_core::Capability::UseOwnSandboxes]),
    );
    let stream = super::stream::open(state.clone(), caller, agent, 3, None);
    assert_eq!(stream.status(), StatusCode::OK);
    assert_eq!(stream.headers()["content-type"], "text/event-stream");
    let bytes = to_bytes(stream.into_body(), 1024 * 1024).await.unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    let values: Vec<Value> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(values.first().unwrap()["type"], "response.created");
    assert_eq!(values.last().unwrap()["type"], "response.completed");
    for (sequence, value) in values.iter().enumerate() {
        assert_eq!(value["sequence_number"], sequence);
    }
    let deltas = values
        .iter()
        .filter(|value| value["type"] == "response.output_text.delta")
        .map(|value| value["delta"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(deltas, ["First answer", "Second answer"]);
    let activity = values
        .iter()
        .position(|value| value["type"] == "response.cma_chat.tool_activity")
        .unwrap();
    let first_done = values
        .iter()
        .position(|value| value["type"] == "response.output_item.done")
        .unwrap();
    let second_added = values
        .iter()
        .rposition(|value| value["type"] == "response.output_item.added")
        .unwrap();
    assert!(first_done < activity && activity < second_added);
    let router = super::router(state);
    let (status, retrieved) = json_response(
        &router,
        request(
            Method::GET,
            "/api/v1/responses/resp_agt_response_3",
            "owner-a",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(values.last().unwrap()["response"], retrieved);
    assert_eq!(retrieved["metadata"]["project"], "proof");
    assert_eq!(retrieved["usage"]["total_tokens"], 17);
    let (_, items) = json_response(
        &router,
        request(
            Method::GET,
            "/api/v1/conversations/agt_response/items?limit=1",
            "owner-a",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(items["data"][0]["id"], retrieved["output"][1]["id"]);
    assert_eq!(items["has_more"], true);
    let after = items["last_id"].as_str().unwrap();
    let (_, next) = json_response(
        &router,
        request(
            Method::GET,
            &format!("/api/v1/conversations/agt_response/items?after={after}&limit=2"),
            "owner-a",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(next["data"][0]["type"], "cma_chat.tool_activity");
    assert_eq!(next["data"][1]["id"], retrieved["output"][0]["id"]);
}

#[tokio::test]
async fn foreign_predecessor_refusals_and_foreign_response_reads_are_typed() {
    let (state, _) = completed().await;
    let router = super::router(state);
    let (status, body) = json_response(
        &router,
        request(
            Method::POST,
            "/api/v1/responses",
            "owner-a",
            None,
            Some(json!({"conversation": "agt_response", "input": "Next", "stream": true, "previous_response_id":"resp_agt_other_3"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "previous_response_id_conflict");
    let foreign = response(
        &router,
        request(
            Method::GET,
            "/api/v1/responses/resp_agt_response_3",
            "owner-b",
            None,
            None,
        ),
    )
    .await;
    let missing = response(
        &router,
        request(
            Method::GET,
            "/api/v1/responses/resp_agt_missing_3",
            "owner-b",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(foreign, missing);
    assert_eq!(foreign.0, StatusCode::NOT_FOUND);
    assert!(super::retrieve::parse_response_id("resp_agt_response_03").is_none());
    assert!(super::retrieve::parse_response_id("resp_sesn_retired_3").is_none());
}
