use super::*;
use tower::ServiceExt;

#[tokio::test]
async fn edited_reply_keeps_exact_body_owner_and_identity_in_public_history() {
    for protocol in ["agui", "responses"] {
        for scoped in [true, false] {
            let (state, store) = crate::rest::agents_tests::conversation_fixture_with_registry(
                std::sync::Arc::new(SuggestionRuntime),
            )
            .await;
            insert_agent(
                &store,
                seeded_agent("agt_edited", AgentStatus::Ready, 6, None),
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
            let edited = "保留预算上限\nCompare latency and cost.";
            let scope = json!({"turn_sequence":3,"answer_sequence":5});
            let (path, mut body) = if protocol == "agui" {
                (
                    "/api/v1/agui/run",
                    json!({"threadId":"agt_edited", "messages":[{"role":"user", "content":edited}]}),
                )
            } else {
                (
                    "/api/v2/responses",
                    json!({"conversation":"agt_edited", "input":edited, "stream":true}),
                )
            };
            if scoped {
                if protocol == "agui" {
                    body["forwardedProps"] = json!({"answerContext":scope});
                } else {
                    body["answer_context"] = scope;
                }
            }
            let forbidden = router
                .clone()
                .oneshot(request(
                    Method::POST,
                    path,
                    "owner-b",
                    Some("edited-intent"),
                    Some(body.clone()),
                ))
                .await
                .unwrap();
            assert_eq!(forbidden.status(), StatusCode::NOT_FOUND);
            let admitted = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                router.clone().oneshot(request(
                    Method::POST,
                    path,
                    "owner-a",
                    Some("edited-intent"),
                    Some(body.clone()),
                )),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(admitted.status(), StatusCode::OK);
            let identity = admitted.headers()["x-cma-command-id"].clone();
            drop(admitted);
            let recovered = router
                .clone()
                .oneshot(request(
                    Method::POST,
                    path,
                    "owner-a",
                    Some("edited-intent"),
                    Some(body.clone()),
                ))
                .await
                .unwrap();
            assert_eq!(recovered.status(), StatusCode::OK);
            assert_eq!(recovered.headers()["x-cma-command-id"], identity);
            drop(recovered);
            if protocol == "agui" {
                body["messages"][0]["content"] = json!("Changed intent must use a fresh key");
            } else {
                body["input"] = json!("Changed intent must use a fresh key");
            }
            let overwritten = router
                .clone()
                .oneshot(request(
                    Method::POST,
                    path,
                    "owner-a",
                    Some("edited-intent"),
                    Some(body),
                ))
                .await
                .unwrap();
            assert_eq!(overwritten.status(), StatusCode::CONFLICT);
            let history_path = "/api/v1/agui/events?thread_id=agt_edited&follow=false";
            let (status, history) = response(
                &router,
                request(Method::GET, history_path, "owner-a", None, None),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let text = String::from_utf8(history.to_vec()).unwrap();
            let frames: Vec<Value> = text
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(|line| serde_json::from_str(line.trim()).unwrap())
                .collect();
            let messages: Vec<_> = frames
                .iter()
                .filter(|frame| frame["type"] == "TEXT_MESSAGE_CONTENT" && frame["delta"] == edited)
                .collect();
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0]["threadId"], "agt_edited");
            assert!(frames
                .iter()
                .any(|frame| frame["type"] == "TEXT_MESSAGE_START"
                    && frame["messageId"] == messages[0]["messageId"]
                    && frame["role"] == "user"));
            assert!(!text.contains("Changed intent must use a fresh key"));
            let (status, denied) = response(
                &router,
                request(Method::GET, history_path, "owner-b", None, None),
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            assert!(!String::from_utf8(denied.to_vec()).unwrap().contains(edited));
        }
    }
}
