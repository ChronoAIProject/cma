use super::*;
use layer2_core::sandbox::suggestions::*;

struct DelayedModel {
    entered: tokio::sync::Notify,
    released: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl SuggestionGenerator for DelayedModel {
    async fn generate(&self, _: &SuggestionWork) -> Result<SuggestionGeneration, SuggestionFailure> {
        self.entered.notify_one();
        self.released.notified().await;
        Ok(SuggestionGeneration {
            output: r#"{"suggestions":[{"text":"Explain SQLite backup options.","evidence":"local-only app"}]}"#.into(),
            usage: SuggestionUsage { model: Some("fixture-model".into()), input_tokens: Some(90), output_tokens: Some(18), ..Default::default() },
        })
    }
}

const READ: &str = "/api/v1/agents/agt_common/suggestions?turn_sequence=3&answer_sequence=5";

async fn completed_fixture() -> Fixture {
    let test = fixture().await;
    let id = AgentId("agt_common".into());
    for kind in [
        AgentLogKind::OwnerMessage { content: "Compare SQLite and PostgreSQL for local storage.".into() },
        AgentLogKind::AgentMessage { content: "SQLite suits a local-only app.".into() },
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ] {
        test.store.append_agent_log(&id, "2026-11-03T00:00:00Z".into(), kind).await.unwrap();
    }
    let mut agent = test.store.get_agent(&id).await.unwrap().unwrap();
    let previous = agent.revision;
    agent.conversation.active_turn = None;
    agent.conversation.last_seq = 6;
    agent.status = AgentStatus::Ready;
    agent.revision += 1;
    test.store.save_agent(previous, agent).await.unwrap();
    test
}

#[tokio::test]
async fn contextual_suggestions_cannot_reopen_answer_a_after_new_input() {
    let test = completed_fixture().await;
    let model = Arc::new(DelayedModel { entered: tokio::sync::Notify::new(), released: tokio::sync::Notify::new() });
    let generation = SuggestionGenerationService::new(test.store.clone());
    let running_model = model.clone();
    let task = tokio::spawn(async move { generation.run_next(running_model.as_ref()).await.unwrap() });
    model.entered.notified().await;
    let (status, body) = agents_tests::json_response(&test.router,
        request("owner-a", "input", "next-message", Some(json!({"text":"Use SQLite."}))),
    ).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    model.released.notify_one();
    assert!(task.await.unwrap());
    let (status, body) = agents_tests::json_response(&test.router,
        agents_tests::request(Method::GET, READ, "owner-a", None, None),
    ).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "expired");
    assert!(body["set"].is_null());
    assert_eq!(body["observation"]["usage"]["input_tokens"], 90);
    let reopened = SuggestionGenerationService::new(test.store.clone());
    assert!(!reopened.run_next(model.as_ref()).await.unwrap());
}
