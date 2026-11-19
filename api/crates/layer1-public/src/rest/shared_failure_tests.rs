use super::*;
use layer2_core::sandbox::store::AgentLogStore;

#[tokio::test]
async fn retained_provider_failure_rest_history_is_safe_for_owner_and_shared_readers() {
    let (fixture, _, _, _) = fixture(Reply::Member).await;
    let id = AgentId("agt_shared_read".into());
    let before = fixture.store.agent_log(&id, 0, 200).await.unwrap();
    for subject in ["owner-a", "recipient"] {
        for suffix in [
            "/events",
            "/transcript",
            "/transcript?limit=2&turn=3&after=8&through=9",
        ] {
            let path = format!("/api/v1/agents/agt_shared_read{suffix}");
            let (status, body) = json_response(&fixture.router, request(&path, subject)).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            let wire = body.to_string();
            assert!(
                !wire.contains("private-host"),
                "retained error leaked: {wire}"
            );
            assert!(
                !wire.contains("runtime_failed"),
                "unapproved class leaked: {wire}"
            );
            assert!(
                wire.contains("agent_turn_failed"),
                "failure missing: {wire}"
            );
            assert!(wire.contains("The Agent run failed."));
        }
    }
    assert_eq!(fixture.store.agent_log(&id, 0, 200).await.unwrap(), before);
}

#[tokio::test]
async fn provider_failure_initial_task_copy_stays_private_on_agent_detail() {
    use layer2_core::sandbox::{FirstMessageFailure, FirstMessageRecord};
    let (fixture, _, _, _) = fixture(Reply::Member).await;
    let mut agent = agents_tests::seeded_agent("agt_first_failure", AgentStatus::Ready, 4, None);
    let at = "2026-09-05T00:00:00Z";
    let mut first = FirstMessageRecord::new("first", "Begin", at).unwrap();
    first.fence_dispatch_for(3, 1, 1, at.into()).unwrap();
    first
        .fail(
            FirstMessageFailure::new("PRIVATE_CLASS_SENTINEL", "PRIVATE_BODY_SENTINEL", false, at)
                .unwrap(),
        )
        .unwrap();
    agent.conversation.first_message = Some(first);
    agents_tests::insert_agent(
        &fixture.store,
        agent,
        vec![
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 3,
                origin_key: Some("first".into()),
            },
            AgentLogKind::TurnFailed {
                turn_sequence: 3,
                failure: TypedFailure {
                    class: "PRIVATE_CLASS_SENTINEL".into(),
                    detail: "PRIVATE_BODY_SENTINEL".into(),
                },
            },
        ],
    )
    .await;
    let (status, body) = json_response(
        &fixture.router,
        request("/api/v1/agents/agt_first_failure", "owner-a"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["first_message"]["state"], "failed");
    assert!(!body.to_string().contains("SENTINEL"));
    assert!(body["first_message"].get("failure").is_none());
}
