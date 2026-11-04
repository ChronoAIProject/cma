use super::*;
use tower::ServiceExt;

#[tokio::test]
async fn task_experience_default_off_refuses_new_answer_scoped_admission() {
    let (state, store) = crate::rest::agents_tests::conversation_fixture_with_registry(
        std::sync::Arc::new(SuggestionRuntime),
    )
    .await;
    insert_agent(
        &store,
        seeded_agent("agt_policy", AgentStatus::Ready, 6, None),
        vec![
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 3,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "Compare databases".into(),
            },
            AgentLogKind::AgentMessage {
                content: "Comparison complete".into(),
            },
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
        ],
    )
    .await;
    let mut state = crate::conversation::command_test_support::compose(state, store.clone());
    state.commands = state
        .commands
        .take()
        .map(|service| service.with_task_experience(Default::default()));
    let router = crate::conversation::command_test_support::router(state, store);
    let body = json!({"conversation":"agt_policy", "input":"Add latency", "stream":true,
        "answer_context":{"turn_sequence":3,"answer_sequence":5}});
    let response = router
        .oneshot(request(
            Method::POST,
            "/api/v2/responses",
            "owner-a",
            Some("policy-new-intent"),
            Some(body),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn task_experience_closure_preserves_exact_admitted_receipt_after_fresh_authorization() {
    for protocol in ["agui", "responses"] {
        let (state, store) = crate::rest::agents_tests::conversation_fixture_with_registry(
            std::sync::Arc::new(SuggestionRuntime),
        )
        .await;
        insert_agent(
            &store,
            seeded_agent("agt_policy", AgentStatus::Ready, 6, None),
            vec![
                AgentLogKind::TurnStarted {
                    model: None,
                    turn_sequence: 3,
                    origin_key: None,
                },
                AgentLogKind::OwnerMessage {
                    content: "Compare databases".into(),
                },
                AgentLogKind::AgentMessage {
                    content: "Comparison complete".into(),
                },
                AgentLogKind::TurnCompleted { turn_sequence: 3 },
            ],
        )
        .await;
        let mut state = crate::conversation::command_test_support::compose(state, store.clone());
        let router =
            crate::conversation::command_test_support::router(state.clone(), store.clone());
        let scope = json!({"turn_sequence":3,"answer_sequence":5});
        let (path, body) = if protocol == "agui" {
            (
                "/api/v1/agui/run",
                json!({"threadId":"agt_policy","messages":[{"role":"user","content":"Keep exact whitespace.  "}],"forwardedProps":{"answerContext":scope}}),
            )
        } else {
            (
                "/api/v2/responses",
                json!({"conversation":"agt_policy","input":"Keep exact whitespace.  ","stream":true,"answer_context":scope}),
            )
        };
        let accepted = router
            .oneshot(request(
                Method::POST,
                path,
                "owner-a",
                Some("policy-original"),
                Some(body.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(accepted.status(), StatusCode::OK);
        let identity = accepted.headers()["x-cma-command-id"].clone();
        drop(accepted);
        state.commands = state
            .commands
            .take()
            .map(|service| service.with_task_experience(Default::default()));
        let closed = crate::conversation::command_test_support::router(state, store);
        for (subject, key, expected) in [
            ("owner-a", "policy-original", StatusCode::OK),
            ("owner-b", "policy-original", StatusCode::NOT_FOUND),
            ("owner-a", "policy-new", StatusCode::FORBIDDEN),
        ] {
            let reply = closed
                .clone()
                .oneshot(request(
                    Method::POST,
                    path,
                    subject,
                    Some(key),
                    Some(body.clone()),
                ))
                .await
                .unwrap();
            assert_eq!(reply.status(), expected);
            if expected == StatusCode::OK {
                assert_eq!(reply.headers()["x-cma-command-id"], identity);
            }
        }
        let mut changed = body;
        if protocol == "agui" {
            changed["messages"][0]["content"] = json!("Different request");
        } else {
            changed["input"] = json!("Different request");
        }
        let reply = closed
            .clone()
            .oneshot(request(
                Method::POST,
                path,
                "owner-a",
                Some("policy-original"),
                Some(changed),
            ))
            .await
            .unwrap();
        assert_eq!(reply.status(), StatusCode::CONFLICT);
        let (status, history) = response(
            &closed,
            request(
                Method::GET,
                "/api/v1/agui/events?thread_id=agt_policy&follow=false",
                "owner-a",
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let text = String::from_utf8(history.to_vec()).unwrap();
        assert!(text.contains("Keep exact whitespace.  "));
        assert!(!text.contains("Different request"));
    }
}
