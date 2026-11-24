use super::*;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use layer2_core::sandbox::agent_io::AnswerContext;
use layer2_core::sandbox::suggestions::*;

#[path = "auxiliary_routes/authority_tests.rs"]
mod authority_tests;
#[path = "auxiliary_routes/route_tests.rs"]
mod route_tests;
#[path = "auxiliary_routes/transport_tests.rs"]
mod transport_tests;

#[derive(Clone)]
enum Reply {
    Success,
    Failure(StatusCode),
    Redirect(StatusCode, String),
}

struct Received {
    uri: String,
    headers: HeaderMap,
    body: serde_json::Value,
}

struct Peer {
    origin: String,
    calls: Arc<Mutex<Vec<Received>>>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Peer {
    async fn start(reply: Reply) -> Self {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let captured = calls.clone();
        let router = Router::new().fallback_service(post(
            move |uri: Uri, headers: HeaderMap, Json(body): Json<serde_json::Value>| {
                let calls = captured.clone();
                let reply = reply.clone();
                async move {
                    calls.lock().unwrap().push(Received {
                        uri: uri.to_string(),
                        headers,
                        body,
                    });
                    match reply {
                        Reply::Success => Json(serde_json::json!({
                            "status":"completed", "model":MODEL,
                            "output_text":"Scoped auxiliary title",
                            "output":[{"type":"message","role":"assistant","content":[{
                                "type":"output_text","text":"{\"suggestions\":[]}"
                            }]}]
                        }))
                        .into_response(),
                        Reply::Failure(status) => {
                            let code = if status == StatusCode::PAYMENT_REQUIRED {
                                "billing_not_active"
                            } else {
                                "upstream_unavailable"
                            };
                            (status, Json(serde_json::json!({"error":{"code":code}})))
                                .into_response()
                        }
                        Reply::Redirect(status, location) => Response::builder()
                            .status(status)
                            .header("location", location)
                            .body(axum::body::Body::empty())
                            .unwrap(),
                    }
                }
            },
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            origin,
            calls,
            server,
        }
    }

    fn endpoint(&self) -> Arc<PlatformTitleEndpoint> {
        Arc::new(PlatformTitleEndpoint {
            platform_base_url: self.origin.clone(),
        })
    }
}

struct Reading;

#[async_trait::async_trait]
impl SuggestionStore for Reading {
    async fn read_suggestions(
        &self,
        _: &SubjectId,
        _: &AgentId,
        _: AnswerContext,
    ) -> Result<SuggestionReading, AgentReadError> {
        Ok(SuggestionReading {
            status: SuggestionStatus::Running,
            set: None,
            observation: None,
        })
    }
    async fn claim_suggestions(&self, _: u64) -> Result<Option<SuggestionWork>, AgentReadError> {
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

fn title_worker(f: &Fixture, endpoint: Arc<dyn TitleRuntimeEndpoint>) -> Arc<AgentTitleWorker> {
    AgentTitleWorker::new(
        f.service.clone(),
        f.store.clone(),
        f.credentials.clone(),
        endpoint,
        MODEL.into(),
        f.budget.clone(),
    )
}

fn suggestion_worker(
    f: &Fixture,
    endpoint: Arc<dyn TitleRuntimeEndpoint>,
) -> Arc<crate::sandbox::suggestion_worker::SuggestionWorker> {
    crate::sandbox::suggestion_worker::SuggestionWorker::with_dependencies(
        f.store.clone(),
        Arc::new(Reading),
        f.credentials.clone(),
        endpoint,
        MODEL.into(),
        Arc::new(
            serde_json::from_value(serde_json::json!({
                "mode":"all","accounts":[],"capabilities":["suggested_followup_admission"]
            }))
            .unwrap(),
        ),
    )
}

async fn suggestion_work(f: &Fixture) -> SuggestionWork {
    let agent = f.store.get_agent(&f.id).await.unwrap().unwrap();
    let sandbox = f
        .store
        .get_sandbox(&agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    SuggestionWork {
        id: "auxiliary-route-proof".into(),
        owner: agent.owner.clone(),
        agent_id: agent.id.clone(),
        scope: AnswerContext {
            turn_sequence: 1,
            answer_sequence: 3,
        },
        completed_sequence: 4,
        strategy_version: STRATEGY_VERSION.into(),
        authority: authority_digest(&agent, &sandbox),
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
    }
}

fn assert_no_legacy_proxy(f: &Fixture) {
    assert_eq!(f.runtime.calls.load(Ordering::SeqCst), 0);
    assert!(f.capture.requests.lock().unwrap().is_empty());
}
