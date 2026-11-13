use super::*;
use serde_json::{json, Value};

#[tokio::test]
async fn soft_deleted_retained_history_remains_identical_until_purge() {
    for active in [false, true] {
        let f = fixture(active, false).await;
        let mut agent = f
            .store
            .get_agent(&AgentId("agt_fixture".into()))
            .await
            .unwrap()
            .unwrap();
        let revision = agent.revision;
        agent.status = AgentStatus::Deleted;
        agent.deleted_at = Some(layer2_core::now_rfc3339());
        agent.revision += 1;
        f.store.save_agent(revision, agent).await.unwrap();
        let on = router(f.state.clone().with_single_fence_pages(true));
        for route in [SNAPSHOT, HEAD] {
            let expected = raw(&f.router, route, "owner-a", "cma-message-bindings").await;
            assert_eq!(expected.0, StatusCode::OK);
            assert_eq!(
                raw(&on, route, "owner-a", "cma-message-bindings").await,
                expected
            );
        }
    }
}

#[tokio::test]
async fn shared_pages_and_heads_preserve_projection_and_entry_refusals() {
    for active in [false, true] {
        let f = fixture(active, false).await;
        super::super::head_tests::share(&f).await;
        let on = router(f.state.clone().with_single_fence_pages(true));
        for route in [SNAPSHOT, HEAD] {
            let expected = raw(&f.router, route, "reader", "cma-message-bindings").await;
            assert_eq!(expected.0, StatusCode::OK);
            assert_eq!(
                raw(&on, route, "reader", "cma-message-bindings").await,
                expected
            );
            let denied = raw(&f.router, route, "foreign", "").await;
            assert_eq!(denied.0, StatusCode::NOT_FOUND);
            assert_eq!(raw(&on, route, "foreign", "").await, denied);
        }
    }
}

#[tokio::test]
async fn page_head_and_stream_bytes_match_with_bindings_cursors_and_tool_preferences() {
    let (f, steering_command) = super::super::message_binding_tests::steered().await;
    let recorder = DurableAgentUpdateRecorder::with_writer_policy(f.store.clone(), task_policy());
    let mut assembly = AgentMessageAssembly::default();
    for update in [
        json!({"sessionUpdate":"tool_call", "toolCallId":"buffered-proof", "title":"read", "kind":"read", "status":"in_progress"}),
        json!({"sessionUpdate":"tool_call_update", "toolCallId":"buffered-proof", "content":[{"type":"content", "content":{"type":"text", "text":"buffered private output"}}]}),
    ] {
        recorder
            .record_update(
                &AgentId("agt_fixture".into()),
                1,
                &mut assembly,
                AgentRuntimeUpdate::Said(serde_json::from_value(update).unwrap()),
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
    }
    for _ in 0..205 {
        append(
            &f.store,
            AgentLogKind::ModeChanged {
                mode: "code".into(),
            },
        )
        .await;
    }
    let on = router(f.state.clone().with_single_fence_pages(true));
    for prefer in [
        "",
        "cma-message-bindings",
        "cma-tool-output",
        "cma-message-bindings, cma-tool-output",
    ] {
        let mut path = format!("{SNAPSHOT}?limit=100");
        let mut pages = 0;
        loop {
            let expected = raw(&f.router, &path, "owner-a", prefer).await;
            assert_eq!(expected.0, StatusCode::OK);
            assert_eq!(raw(&on, &path, "owner-a", prefer).await, expected);
            let text = String::from_utf8_lossy(&expected.1);
            if pages == 0 {
                assert!(text.contains("resp_control_source"));
                assert_eq!(
                    text.contains(&steering_command),
                    prefer.contains("cma-message-bindings")
                );
                assert_eq!(
                    text.contains("buffered private output"),
                    prefer.contains("cma-tool-output")
                );
            }
            pages += 1;
            let body: Value = serde_json::from_slice(&expected.1).unwrap();
            match body["next_page_cursor"].as_str() {
                Some(cursor) => path = format!("{SNAPSHOT}?page_cursor={cursor}"),
                None => break,
            }
        }
        assert_eq!(pages, 3);
        for path in [HEAD.to_owned(), format!("{EVENTS}?follow=false")] {
            assert_eq!(
                raw(&on, &path, "owner-a", prefer).await,
                raw(&f.router, &path, "owner-a", prefer).await
            );
        }
    }
    for path in [
        format!("{SNAPSHOT}?page_cursor=broken"),
        format!("{SNAPSHOT}?limit=0"),
        format!("{HEAD}?limit=1"),
    ] {
        let off = raw(&f.router, &path, "owner-a", "").await;
        assert_eq!(off.0, StatusCode::BAD_REQUEST);
        assert_eq!(raw(&on, &path, "owner-a", "").await, off);
    }
    let mut agent = f
        .store
        .get_agent(&AgentId("agt_fixture".into()))
        .await
        .unwrap()
        .unwrap();
    let revision = agent.revision;
    agent.conversation.active_turn = None;
    agent.status = AgentStatus::Ready;
    agent.revision += 1;
    f.store.save_agent(revision, agent).await.unwrap();
    assert_eq!(
        raw(&on, HEAD, "owner-a", "").await,
        raw(&f.router, HEAD, "owner-a", "").await
    );
}

#[tokio::test]
async fn enabled_pages_retain_byte_bound_and_reject_extra_tail_and_invalid_active_pointer() {
    let f = fixture(true, false).await;
    for _ in 0..5 {
        append(
            &f.store,
            AgentLogKind::AgentMessage {
                content: "x".repeat(1024 * 1024),
            },
        )
        .await;
    }
    let on = router(f.state.clone().with_single_fence_pages(true));
    let off = raw(&f.router, SNAPSHOT, "owner-a", "").await;
    assert_eq!(off.0, StatusCode::OK);
    assert!(off.1.len() <= 4 * 1024 * 1024);
    assert_eq!(raw(&on, SNAPSHOT, "owner-a", "").await, off);
    let body: Value = serde_json::from_slice(&off.1).unwrap();
    assert!(body["next_page_cursor"].is_string());
    f.store
        .append_agent_log(
            &AgentId("agt_fixture".into()),
            layer2_core::now_rfc3339(),
            AgentLogKind::ModeChanged {
                mode: "code".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        raw(&on, SNAPSHOT, "owner-a", "").await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );

    for turn in [0, 1, 999] {
        let f = fixture(true, false).await;
        let mut agent = f
            .store
            .get_agent(&AgentId("agt_fixture".into()))
            .await
            .unwrap()
            .unwrap();
        let revision = agent.revision;
        agent.conversation.active_turn = Some(turn);
        agent.revision += 1;
        f.store.save_agent(revision, agent).await.unwrap();
        let on = router(f.state.clone().with_single_fence_pages(true));
        assert_eq!(
            raw(&on, HEAD, "owner-a", "").await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
