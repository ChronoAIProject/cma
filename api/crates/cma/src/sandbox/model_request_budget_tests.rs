use super::*;
use axum::{routing::post, Json, Router};
use layer2_core::sandbox::suggestions::*;
use layer2_core::sandbox::*;
use layer2_core::{AgentId, SubjectId};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

struct Budget {
    failure: Option<BudgetError>,
    reserved: AtomicUsize,
    settled: Mutex<Vec<RequestUsage>>,
    holds: Mutex<Vec<RequestHold>>,
}

#[async_trait::async_trait]
impl RequestBudget for Budget {
    async fn reserve_request(
        &self,
        scope: &RequestScope,
        request: &ModelRequest,
    ) -> Result<Option<RequestHold>, BudgetError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        self.reserved.fetch_add(1, Ordering::SeqCst);
        assert_eq!(request.max_output_tokens, 1024);
        assert!(request.input_bytes > 0);
        let hold = RequestHold {
            scope: scope.clone(),
            request: request.clone(),
            input_tokens_bound: request.input_bytes + 64,
        };
        self.holds.lock().unwrap().push(hold.clone());
        Ok(Some(hold))
    }
    async fn settle_request(
        &self,
        _: &RequestHold,
        usage: RequestUsage,
    ) -> Result<(), BudgetError> {
        self.settled.lock().unwrap().push(usage);
        Ok(())
    }
}

fn budget(failure: Option<BudgetError>) -> Arc<Budget> {
    Arc::new(Budget {
        failure,
        reserved: AtomicUsize::new(0),
        settled: Mutex::new(Vec::new()),
        holds: Mutex::new(Vec::new()),
    })
}

#[path = "model_request_budget_tests/durable_http.rs"]
pub(crate) mod durable_http;
#[path = "model_request_budget_tests/uncertain_http.rs"]
mod uncertain_http;

fn context(budget: Arc<Budget>, kind: &str) -> RequestBudgetContext {
    RequestBudgetContext {
        budget,
        scope: RequestScope {
            agent_id: AgentId("agt_request_budget".into()),
            turn_sequence: 1,
            request_id: kind.into(),
        },
    }
}

async fn server(
    budget: Arc<Budget>,
    usage: bool,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let recorded = calls.clone();
    let app = Router::new().route(
        "/v1/responses",
        post(move || {
            let budget = budget.clone();
            let calls = recorded.clone();
            async move {
                assert!(
                    budget.reserved.load(Ordering::SeqCst) > calls.fetch_add(1, Ordering::SeqCst)
                );
                let mut response = serde_json::json!({"status":"completed", "model":"budget-model",
                "output_text":"Reserved title", "output":[{"type":"message","role":"assistant",
                "content":[{"type":"output_text","text":"{\"suggestions\":[]}"}]}]});
                if usage {
                    response["usage"] = serde_json::json!({"input_tokens": 7, "output_tokens": 3});
                }
                Json(response)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (origin, calls, task)
}

async fn title(
    origin: &str,
    budget: Arc<Budget>,
) -> Result<AgentTitleOutcome, AgentTitleServiceError> {
    title_with_context(origin, context(budget, "title-proof")).await
}

async fn title_with_context(
    origin: &str,
    budget: RequestBudgetContext,
) -> Result<AgentTitleOutcome, AgentTitleServiceError> {
    let id = budget.scope.agent_id.clone();
    let turn_sequence = budget.scope.turn_sequence;
    let store = Arc::new(InMemoryAgentTitleStore::default());
    store.register_agent(id.clone()).unwrap();
    for kind in [
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "Explain reservations".into(),
        },
        AgentLogKind::AgentMessage {
            content: "Reserve before requesting".into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence },
    ] {
        store
            .append_agent_log(&id, "2026-11-11T00:00:00Z", kind)
            .unwrap();
    }
    let service = AgentTitleService::new(store, "budget-holder").unwrap();
    let generator = crate::sandbox::titles::HttpAgentTitleGenerator::new(
        &layer3_adapter::provider::ProviderEndpoint {
            endpoint: origin.into(),
        },
        "budget-model",
    )
    .unwrap()
    .with_request_budget(budget);
    service
        .generate_agent_title(
            &id,
            Some(&AgentTitleGenerationContext::new("budget-model", "budget-test-bearer").unwrap()),
            Some(&generator),
        )
        .await
}

struct MeteredRefusals {
    inner: Arc<dyn RequestBudget>,
    reason: BudgetError,
    count: AtomicUsize,
}

#[async_trait::async_trait]
impl RequestBudget for MeteredRefusals {
    async fn reserve_request(
        &self,
        scope: &RequestScope,
        request: &ModelRequest,
    ) -> Result<Option<RequestHold>, BudgetError> {
        let outcome = self.inner.reserve_request(scope, request).await;
        if outcome == Err(self.reason) {
            self.count.fetch_add(1, Ordering::SeqCst);
        }
        outcome
    }

    async fn settle_request(
        &self,
        hold: &RequestHold,
        usage: RequestUsage,
    ) -> Result<(), BudgetError> {
        self.inner.settle_request(hold, usage).await
    }
}

/// Exercise both HTTP adapters with the Mongo reservation implementation.
pub(crate) async fn assert_metered_auxiliary_refusal(
    budget: Arc<dyn RequestBudget>,
    scope: RequestScope,
    reason: BudgetError,
) {
    let observed = Arc::new(MeteredRefusals {
        inner: budget,
        reason,
        count: AtomicUsize::new(0),
    });
    let calls = Arc::new(AtomicUsize::new(0));
    let captured = calls.clone();
    let app = Router::new().route(
        "/v1/responses",
        post(move || {
            let calls = captured.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Json(serde_json::json!({"output_text": "Unreserved title"}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let title_result = title_with_context(
        &origin,
        RequestBudgetContext {
            budget: observed.clone(),
            scope: scope.clone(),
        },
    )
    .await;
    let suggestion_result = suggestion_with_context(
        &origin,
        RequestBudgetContext {
            budget: observed.clone(),
            scope: RequestScope {
                request_id: format!("{}-suggestion", scope.request_id),
                ..scope
            },
        },
    )
    .await;
    server.abort();
    assert_eq!(
        observed.count.load(Ordering::SeqCst),
        2,
        "both HTTP adapters must reach the expected Mongo budget refusal"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "no model request without a valued hold"
    );
    assert_eq!(title_result, Err(AgentTitleServiceError::Budget(reason)));
    assert!(matches!(
        suggestion_result,
        Err(SuggestionFailure::Unavailable)
    ));
}

async fn suggestion(
    origin: &str,
    budget: Arc<Budget>,
) -> Result<SuggestionGeneration, SuggestionFailure> {
    suggestion_with_context(origin, context(budget, "suggestion-proof")).await
}

async fn suggestion_with_context(
    origin: &str,
    budget: RequestBudgetContext,
) -> Result<SuggestionGeneration, SuggestionFailure> {
    let work = SuggestionWork {
        id: budget.scope.request_id.clone(),
        owner: SubjectId("budget-owner".into()),
        agent_id: budget.scope.agent_id.clone(),
        scope: agent_io::AnswerContext {
            turn_sequence: budget.scope.turn_sequence,
            answer_sequence: 3,
        },
        completed_sequence: 4,
        strategy_version: STRATEGY_VERSION.into(),
        authority: "proof".into(),
        status: SuggestionStatus::Running,
        context: SuggestionContext {
            goal: "Explain reservations".into(),
            answer: "Reserve first".into(),
            confirmed_resources: Vec::new(),
        },
        set: None,
        created_at_ms: 0,
        started_at_ms: Some(0),
        deadline_ms: Some(1),
        observation: None,
    };
    let generator = crate::sandbox::suggestions::HttpSuggestionGenerator::new(
        origin,
        "budget-model",
        crate::auth::persistence::SecretValue::new("budget-test-bearer".into()),
        None,
    )
    .unwrap()
    .with_request_budget(budget);
    generator.generate(&work).await
}

#[tokio::test]
async fn request_budget_refusals_never_send_title_or_suggestion_http_requests() {
    for reason in [
        BudgetError::Unavailable,
        BudgetError::OwnerExhausted,
        BudgetError::TurnExhausted,
    ] {
        let budget = budget(Some(reason));
        let (origin, calls, task) = server(budget.clone(), true).await;
        assert_eq!(
            title(&origin, budget.clone()).await,
            Err(AgentTitleServiceError::Budget(reason))
        );
        assert!(matches!(
            suggestion(&origin, budget.clone()).await,
            Err(SuggestionFailure::Unavailable)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(budget.reserved.load(Ordering::SeqCst), 0);
        task.abort();
    }
}

#[tokio::test]
async fn brokered_requests_hold_before_send_and_settle_complete_recorded_usage() {
    let budget = budget(None);
    let (origin, calls, task) = server(budget.clone(), true).await;
    assert_eq!(
        title(&origin, budget.clone()).await.unwrap(),
        AgentTitleOutcome::Generated
    );
    assert!(suggestion(&origin, budget.clone()).await.is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        *budget.settled.lock().unwrap(),
        vec![
            RequestUsage {
                input_tokens: 7,
                output_tokens: 3
            };
            2
        ]
    );
    task.abort();
}

#[tokio::test]
async fn missing_final_usage_preserves_every_brokered_request_hold() {
    let budget = budget(None);
    let (origin, calls, task) = server(budget.clone(), false).await;
    assert_eq!(
        title(&origin, budget.clone()).await.unwrap(),
        AgentTitleOutcome::Generated
    );
    assert!(suggestion(&origin, budget.clone()).await.is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(budget.reserved.load(Ordering::SeqCst), 2);
    assert!(budget.settled.lock().unwrap().is_empty());
    task.abort();
}

#[tokio::test]
async fn usage_without_matching_model_or_finality_retains_the_request_hold() {
    for response in [
        serde_json::json!({"status":"completed", "usage":{"input_tokens":1,"output_tokens":1}}),
        serde_json::json!({"status":"completed", "model":"different", "usage":{"input_tokens":1,"output_tokens":1}}),
        serde_json::json!({"status":"incomplete", "model":"budget-model", "usage":{"input_tokens":1,"output_tokens":1}}),
        serde_json::json!({"status":"completed", "model":"budget-model", "usage":{"input_tokens":1}}),
    ] {
        let budget = budget(None);
        let context = context(budget.clone(), "uncertain-response");
        let permit = context
            .reserve(br#"{"model":"budget-model","max_output_tokens":1024}"#)
            .await
            .unwrap()
            .unwrap();
        permit
            .settle(&serde_json::to_vec(&response).unwrap())
            .await
            .unwrap();
        assert!(budget.settled.lock().unwrap().is_empty());
    }
}
