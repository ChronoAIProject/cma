use super::*;

const READ: &str = "/api/v1/agents/agt_suggestions/suggestions?turn_sequence=3&answer_sequence=5";

#[tokio::test]
async fn contextual_suggestions_read_pending_without_starting_another_turn() {
    let mut fixture = fixture().await;
    fixture.state = fixture.state.with_suggestions(fixture.store.clone());
    fixture.router = agent_router(fixture.state.clone());
    insert_agent(&fixture.store, seeded_agent("agt_suggestions", AgentStatus::Ready, 6, None), vec![
        AgentLogKind::TurnStarted { turn_sequence: 3, origin_key: None, model: None },
        AgentLogKind::OwnerMessage { content: "Compare SQLite and PostgreSQL for local storage.".into() },
        AgentLogKind::AgentMessage { content: "SQLite suits a local-only app.".into() },
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ]).await;
    let transcript = "/api/v1/agents/agt_suggestions/transcript";
    let before = json_response(&fixture.router, request(Method::GET, transcript, OWNER, None, None)).await;
    let (status, body) = json_response(&fixture.router, request(Method::GET, READ, OWNER, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "pending");
    assert!(body["set"].is_null());
    let after = json_response(&fixture.router, request(Method::GET, transcript, OWNER, None, None)).await;
    assert_eq!(before, after);
}

