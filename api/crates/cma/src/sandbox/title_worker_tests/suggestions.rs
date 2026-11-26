use super::*;

#[tokio::test]
async fn suggestions_use_the_scoped_platform_endpoint_with_the_real_request_contract() {
    use layer2_core::sandbox::agent_io::AnswerContext;
    use layer2_core::sandbox::suggestions::*;
    struct Reading;
    #[async_trait::async_trait]
    impl SuggestionStore for Reading {
        async fn read_suggestions(
            &self,
            owner: &SubjectId,
            agent: &AgentId,
            scope: AnswerContext,
        ) -> Result<SuggestionReading, AgentReadError> {
            assert_eq!(owner.as_str(), "owner");
            assert_eq!(agent.as_str(), "agt_title");
            assert_eq!(scope.answer_sequence, 3);
            Ok(SuggestionReading {
                status: SuggestionStatus::Running,
                set: None,
                observation: None,
            })
        }
        async fn claim_suggestions(
            &self,
            _: u64,
        ) -> Result<Option<SuggestionWork>, AgentReadError> {
            unreachable!()
        }
        async fn complete_suggestions(
            &self,
            _: &SuggestionWork,
            _: SuggestionCompletion,
        ) -> Result<(), AgentReadError> {
            unreachable!()
        }
    }
    async fn reply(
        State(calls): State<Arc<AtomicUsize>>,
        headers: HeaderMap,
        Json(body): Json<serde_json::Value>,
    ) -> Json<serde_json::Value> {
        assert_eq!(headers["authorization"], format!("Bearer {KEY}"));
        assert!(!headers.contains_key("open-sandbox-api-key"));
        assert_eq!(body["model"], MODEL);
        assert_eq!(body["stream"], false);
        assert_eq!(body["store"], false);
        assert_eq!(body["max_output_tokens"], 1024);
        assert_eq!(body["tool_choice"], "none");
        assert_eq!(body["tools"], serde_json::json!([]));
        assert_eq!(body["text"]["format"]["type"], "json_schema");
        assert_eq!(body["text"]["format"]["strict"], true);
        let context: serde_json::Value =
            serde_json::from_str(body["input"][0]["content"].as_str().unwrap()).unwrap();
        assert_eq!(context["goal"], "Compare options");
        assert_eq!(context["answer"], "A comparison");
        assert_eq!(context["confirmed_resources"], serde_json::json!([]));
        calls.fetch_add(1, Ordering::SeqCst);
        Json(
            serde_json::json!({"status":"completed", "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"{\"suggestions\":[]}"}]}]}),
        )
    }
    let f = Fixture::new(Capture::default(), None).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = Arc::new(PlatformTitleEndpoint {
        platform_base_url: format!("http://{}", listener.local_addr().unwrap()),
    });
    let router = Router::new()
        .route("/api/v1/proxy/s/chrono-llm/responses", post(reply))
        .with_state(calls.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let worker = crate::sandbox::suggestion_worker::SuggestionWorker::with_dependencies(f.store.clone(), Arc::new(Reading), f.credentials.clone(), endpoint, MODEL.into(),
        Arc::new(serde_json::from_value(serde_json::json!({"mode":"all","accounts":[],"capabilities":["suggested_followup_admission"]})).unwrap()));
    let work = SuggestionWork {
        id: "suggestion-worker-proof".into(),
        owner: SubjectId::new("owner"),
        agent_id: f.id.clone(),
        scope: AnswerContext {
            turn_sequence: 1,
            answer_sequence: 3,
        },
        completed_sequence: 4,
        strategy_version: STRATEGY_VERSION.into(),
        authority: authority_digest(&agent(), &sandbox()),
        status: SuggestionStatus::Running,
        context: SuggestionContext {
            goal: "Compare options".into(),
            answer: "A comparison".into(),
            confirmed_resources: vec![],
        },
        set: None,
        created_at_ms: unix_millis(),
        started_at_ms: None,
        deadline_ms: None,
        observation: None,
    };
    let result = worker.generate(&work).await.unwrap();
    assert_eq!(result.output, "{\"suggestions\":[]}");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    server.abort();
}
