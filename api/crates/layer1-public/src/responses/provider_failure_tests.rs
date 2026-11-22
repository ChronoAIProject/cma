use axum::{
    body::to_bytes,
    http::{Method, StatusCode},
};
use layer2_core::sandbox::{AgentLogKind, AgentStatus, TypedFailure};

use crate::rest::agents_tests::{
    conversation_fixture, insert_agent, request, response, seeded_agent,
};

#[tokio::test]
async fn retained_provider_failure_is_safe_on_legacy_get_stream_and_agui_replay() {
    for class in ["agent_turn_failed", "PRIVATE_CLASS_SENTINEL"] {
        let (state, store) = conversation_fixture().await;
        let agent = seeded_agent("agt_safe_failure", AgentStatus::Ready, 6, None);
        insert_agent(&store, agent.clone(), vec![
            AgentLogKind::TurnStarted { model: None, turn_sequence: 3, origin_key: None },
            AgentLogKind::OwnerMessage { content: "Begin".into() },
            AgentLogKind::AgentMessage { content: "<p>503 is valid assistant content</p>".into() },
            AgentLogKind::TurnFailed { turn_sequence: 3, failure: TypedFailure {
                class: class.into(),
                detail: "unexpected status 503: <html>PRIVATE_BODY_SENTINEL https://failure.invalid X-Private-Header</html>".into(),
            } },
        ]).await;
        let router = super::router(state.clone()).merge(crate::agui::router(state.clone()));
        for path in [
            "/api/v1/responses/resp_agt_safe_failure_3",
            "/api/v1/agui/events?thread_id=agt_safe_failure&follow=false",
        ] {
            let (status, bytes) =
                response(&router, request(Method::GET, path, "owner-a", None, None)).await;
            assert_eq!(status, StatusCode::OK);
            assert_safe(&bytes);
        }
        let caller = layer2_core::Caller::new(
            layer2_core::SubjectId("owner-a".into()),
            layer2_core::Authorization::new([layer2_core::Capability::UseOwnSandboxes]),
        );
        let stream = super::stream::open(state, caller, agent, 3, None);
        let bytes = to_bytes(stream.into_body(), 1024 * 1024).await.unwrap();
        assert_safe(&bytes);
    }
}

fn assert_safe(bytes: &[u8]) {
    let wire = std::str::from_utf8(bytes).unwrap();
    for private in [
        "PRIVATE_BODY_SENTINEL",
        "PRIVATE_CLASS_SENTINEL",
        "failure.invalid",
        "X-Private-Header",
    ] {
        assert!(
            !wire.contains(private),
            "public payload exposed {private}: {wire}"
        );
    }
    assert!(wire.contains("agent_turn_failed"));
    assert!(wire.contains("The Agent run failed."));
    assert!(wire.contains("503 is valid assistant content"));
}
