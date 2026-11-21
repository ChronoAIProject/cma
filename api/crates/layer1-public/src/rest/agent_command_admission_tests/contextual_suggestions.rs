use super::*;
use layer2_core::sandbox::suggestions::*;

struct DelayedModel {
    entered: tokio::sync::Notify,
    released: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl SuggestionGenerator for DelayedModel {
    async fn generate(
        &self,
        _: &SuggestionWork,
    ) -> Result<SuggestionGeneration, SuggestionFailure> {
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
    completed_with(
        "Compare SQLite and PostgreSQL for local storage.",
        "SQLite suits a local-only app.",
    )
    .await
}

async fn completed_with(goal: &str, answer: &str) -> Fixture {
    let test = fixture().await;
    let id = AgentId("agt_common".into());
    for kind in [
        AgentLogKind::OwnerMessage {
            content: goal.into(),
        },
        AgentLogKind::AgentMessage {
            content: answer.into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ] {
        test.store
            .append_agent_log(&id, "2026-11-03T00:00:00Z".into(), kind)
            .await
            .unwrap();
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
    let model = Arc::new(DelayedModel {
        entered: tokio::sync::Notify::new(),
        released: tokio::sync::Notify::new(),
    });
    let generation = SuggestionGenerationService::new(test.store.clone());
    let running_model = model.clone();
    let task =
        tokio::spawn(async move { generation.run_next(running_model.as_ref()).await.unwrap() });
    model.entered.notified().await;
    let (status, body) = agents_tests::json_response(
        &test.router,
        request(
            "owner-a",
            "input",
            "next-message",
            Some(json!({"text":"Use SQLite."})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    model.released.notify_one();
    assert!(task.await.unwrap());
    let (status, body) = agents_tests::json_response(
        &test.router,
        agents_tests::request(Method::GET, READ, "owner-a", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "expired");
    assert!(body["set"].is_null());
    assert_eq!(body["observation"]["usage"]["input_tokens"], 90);
    let reopened = SuggestionGenerationService::new(test.store.clone());
    assert!(!reopened.run_next(model.as_ref()).await.unwrap());
}

struct OutputModel(&'static str);

#[async_trait::async_trait]
impl SuggestionGenerator for OutputModel {
    async fn generate(
        &self,
        _: &SuggestionWork,
    ) -> Result<SuggestionGeneration, SuggestionFailure> {
        Ok(SuggestionGeneration {
            output: self.0.into(),
            usage: SuggestionUsage {
                input_tokens: Some(40),
                ..Default::default()
            },
        })
    }
}

async fn generated(output: &'static str) -> Value {
    let test = completed_fixture().await;
    let service = SuggestionGenerationService::new(test.store.clone());
    assert!(service.run_next(&OutputModel(output)).await.unwrap());
    let (status, saved) = agents_tests::json_response(
        &test.router,
        agents_tests::request(Method::GET, READ, "owner-a", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!service.run_next(&OutputModel(output)).await.unwrap());
    let reopened = agents_tests::json_response(
        &test.router,
        agents_tests::request(Method::GET, READ, "owner-a", None, None),
    )
    .await
    .1;
    assert_eq!(saved, reopened);
    saved
}

#[tokio::test]
async fn contextual_suggestions_save_one_exact_candidate_after_duplicate_and_resource_filtering() {
    let saved = generated(r#"{"suggestions":[{"text":"Explain SQLite backup options.","evidence":"local-only app"},{"text":"Explain SQLite backup options!","evidence":"local-only app"},{"text":"Download the published notes.pdf.","evidence":"local-only app"}]}"#).await;
    assert_eq!(saved["status"], "ready");
    assert_eq!(saved["set"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        saved["set"]["items"][0]["text"],
        "Explain SQLite backup options."
    );
    assert_eq!(saved["set"]["items"][0]["rank"], 0);
    assert_eq!(saved["set"]["answerId"], "event:5");
    assert_eq!(saved["set"]["source"]["version"], STRATEGY_VERSION);
}

#[tokio::test]
async fn contextual_suggestions_reject_generic_multi_intent_and_invented_constraints_without_filler(
) {
    let saved = generated(r#"{"suggestions":[{"text":"Continue","evidence":"local-only app"},{"text":"Explain SQLite and implement the storage.","evidence":"local-only app"},{"text":"Limit maintenance to 2 hours daily.","evidence":"local-only app"}]}"#).await;
    assert_eq!(saved["status"], "empty");
    assert!(saved["set"].is_null());
    assert_eq!(saved["observation"]["usage"]["input_tokens"], 40);
    assert!(saved["observation"]["usage"]["cost"].is_null());
}

#[tokio::test]
async fn contextual_suggestions_invalid_model_shape_fails_without_retry() {
    let saved = generated(r#"{"suggestions":[{"text":"Explain the choice."}]}"#).await;
    assert_eq!(saved["status"], "failed");
    assert_eq!(saved["observation"]["failure"], "invalid_response");
}

#[tokio::test(start_paused = true)]
async fn contextual_suggestions_timeout_is_terminal_and_preserves_manual_reads() {
    let test = completed_fixture().await;
    let model = DelayedModel {
        entered: tokio::sync::Notify::new(),
        released: tokio::sync::Notify::new(),
    };
    let service = SuggestionGenerationService::new(test.store.clone());
    assert!(service.run_next(&model).await.unwrap());
    let saved = agents_tests::json_response(
        &test.router,
        agents_tests::request(Method::GET, READ, "owner-a", None, None),
    )
    .await
    .1;
    assert_eq!(saved["status"], "failed");
    assert_eq!(saved["observation"]["failure"], "timeout");
    assert!(!service.run_next(&model).await.unwrap());
}

#[tokio::test]
async fn contextual_suggestions_changed_authority_discards_a_late_completion() {
    let test = completed_fixture().await;
    let work = test
        .store
        .claim_suggestions(unix_millis())
        .await
        .unwrap()
        .unwrap();
    let mut agent = test.store.get_agent(&work.agent_id).await.unwrap().unwrap();
    let revision = agent.revision;
    agent.conversation.runtime_generation = Some(99);
    agent.revision += 1;
    test.store.save_agent(revision, agent).await.unwrap();
    test.store
        .complete_suggestions(
            &work,
            SuggestionCompletion {
                status: SuggestionStatus::Empty,
                set: None,
                observation: SuggestionObservation {
                    elapsed_ms: 1,
                    usage: Default::default(),
                    failure: None,
                },
            },
        )
        .await
        .unwrap();
    let saved = agents_tests::json_response(
        &test.router,
        agents_tests::request(Method::GET, READ, "owner-a", None, None),
    )
    .await
    .1;
    assert_eq!(saved["status"], "expired");
    assert!(saved["set"].is_null());
}

#[tokio::test]
async fn contextual_suggestions_skip_oversized_or_missing_final_context_without_starting_generation(
) {
    for (goal, answer) in [
        ("x".repeat(4097), "A bounded answer".into()),
        ("A bounded goal".into(), "x".repeat(8193)),
        ("A bounded goal".into(), String::new()),
    ] {
        let test = completed_with(&goal, &answer).await;
        assert!(!SuggestionGenerationService::new(test.store.clone())
            .run_next(&OutputModel("must never run"))
            .await
            .unwrap());
        let saved = agents_tests::json_response(
            &test.router,
            agents_tests::request(Method::GET, READ, "owner-a", None, None),
        )
        .await
        .1;
        assert_eq!(saved["status"], "unavailable");
        assert!(saved["set"].is_null());
    }
}
