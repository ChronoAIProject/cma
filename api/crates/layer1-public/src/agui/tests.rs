use axum::http::{Method, StatusCode};
use layer2_core::sandbox::{AgentLogKind, AgentStatus};
use serde_json::{json, Value};

use crate::rest::agents_tests::{
    conversation_fixture, insert_agent, json_response, request, response, seeded_agent,
    seeded_worktree,
};

struct SuggestionRuntime;

mod suggestion_edit;

#[async_trait::async_trait]
impl layer2_core::sandbox::AgentRuntimeRegistry for SuggestionRuntime {
    async fn resolve(
        &self,
        _: &layer2_core::sandbox::AgentRuntimeTarget,
    ) -> Result<layer2_core::sandbox::AgentRuntimeConnection, layer2_core::sandbox::AgentRuntimeError>
    {
        Ok(layer2_core::sandbox::AgentRuntimeConnection {
            capabilities: layer2_core::RuntimeCapabilities::new([
                layer2_core::RuntimeCapability::Conversation,
            ]),
            input: layer2_core::sandbox::AgentRuntimeInputSender::spawn(8, |_| async { Ok(()) }),
        })
    }

    async fn fence(
        &self,
        _: &layer2_core::AgentId,
        _: u64,
    ) -> Result<(), layer2_core::sandbox::AgentRuntimeError> {
        Ok(())
    }
}

#[tokio::test]
async fn suggested_reply_recovers_one_exact_message_after_the_receipt_is_lost() {
    for protocol in ["agui", "responses"] {
        let (state, store) = crate::rest::agents_tests::conversation_fixture_with_registry(
            std::sync::Arc::new(SuggestionRuntime),
        )
        .await;
        insert_agent(
            &store,
            seeded_agent("agt_suggestion", AgentStatus::Ready, 6, None),
            vec![
                AgentLogKind::TurnStarted {
                    model: None,
                    turn_sequence: 3,
                    origin_key: None,
                },
                AgentLogKind::OwnerMessage {
                    content: "Compare PostgreSQL and SQLite.".into(),
                },
                AgentLogKind::AgentMessage {
                    content: "The comparison is complete.".into(),
                },
                AgentLogKind::TurnCompleted { turn_sequence: 3 },
            ],
        )
        .await;
        let state = crate::conversation::command_test_support::compose(state, store.clone());
        let router = crate::conversation::command_test_support::router(state, store);
        let scope = json!({"turn_sequence":3,"answer_sequence":5});
        let (path, body) = if protocol == "agui" {
            (
                "/api/v1/agui/run",
                json!({"threadId":"agt_suggestion", "messages":[{"role":"user", "content":"Add criteria.\nKeep the café limits.  "}],
            "forwardedProps":{"answerContext":scope}}),
            )
        } else {
            (
                "/api/v2/responses",
                json!({"conversation":"agt_suggestion", "input":"Add criteria.\nKeep the café limits.  ",
            "stream":true, "answer_context":scope}),
            )
        };
        use tower::ServiceExt;
        let first = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            router.clone().oneshot(request(
                Method::POST,
                path,
                "owner-a",
                Some("suggestion-one"),
                Some(body.clone()),
            )),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let identity = first.headers()["x-cma-command-id"].clone();
        drop(first);
        let stale = router
            .clone()
            .oneshot(request(
                Method::POST,
                path,
                "owner-a",
                Some("suggestion-other-tab"),
                Some(body.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(stale.status(), StatusCode::CONFLICT);
        let forbidden = router
            .clone()
            .oneshot(request(
                Method::POST,
                path,
                "owner-b",
                Some("suggestion-one"),
                Some(body.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(forbidden.status(), StatusCode::NOT_FOUND);
        let replay = router
            .clone()
            .oneshot(request(
                Method::POST,
                path,
                "owner-a",
                Some("suggestion-one"),
                Some(body),
            ))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        assert_eq!(replay.headers()["x-cma-command-id"], identity);
        drop(replay);
        let (status, history) = response(
            &router,
            request(
                Method::GET,
                "/api/v1/agui/events?thread_id=agt_suggestion&follow=false",
                "owner-a",
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            String::from_utf8(history.to_vec())
                .unwrap()
                .matches("Add criteria.\\nKeep the café limits.  \"")
                .count(),
            1
        );
    }
}

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
                model: None,
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
    let mut frames = text
        .split("\n\n")
        .filter(|frame| frame.contains("data:"))
        .collect::<Vec<_>>();
    assert_eq!(frames.len(), 9);
    let opened = frames.remove(0);
    assert!(opened.contains("id: 2"));
    assert!(opened.contains("\"runId\":\"0\""));
    assert!(opened.contains("\"path\":\"/signIn\""));
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
async fn startup_sign_in_replays_and_clears_without_inventing_a_turn() {
    use layer2_core::sandbox::store::{AgentLogStore, AgentStore, WorktreeStore};

    let (state, store) = conversation_fixture().await;
    let agent = seeded_agent("agt_sign_in", AgentStatus::Ready, 3, None);
    store
        .create_worktree(seeded_worktree(&agent))
        .await
        .unwrap();
    store.create_agent(agent.clone()).await.unwrap();
    for kind in [
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
        AgentLogKind::SignInRequired {
            url: "https://login.example.test/device".into(),
            code: Some("fixture-code".into()),
            message: None,
        },
        AgentLogKind::ConversationOpened {
            conversation_id: "conversation-agt_sign_in".into(),
            runtime_generation: 1,
        },
    ] {
        store
            .append_agent_log(&agent.id, agent.updated_at.clone(), kind)
            .await
            .unwrap();
    }
    let router = super::router(state);
    for (after, expected_count) in [(1, 2), (2, 1)] {
        let (status, body) = response(
            &router,
            request(
                Method::GET,
                &format!(
                    "/api/v1/agui/events?thread_id=agt_sign_in&starting_after={after}&follow=false"
                ),
                "owner-a",
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let text = std::str::from_utf8(&body).unwrap();
        let values = text
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .map(|data| serde_json::from_str::<Value>(data).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(values.len(), expected_count);
        assert!(values.iter().all(|value| value["type"] == "STATE_DELTA"
            && value["runId"] == "0"
            && value["delta"][0]["path"] == "/signIn"));
        if after == 1 {
            assert_eq!(values[0]["delta"][0]["value"]["code"], "fixture-code");
        }
        assert!(values.last().unwrap()["delta"][0]["value"].is_null());
        assert!(text.contains("id: 3"));
        assert!(!text.contains("RUN_STARTED"));
    }
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
