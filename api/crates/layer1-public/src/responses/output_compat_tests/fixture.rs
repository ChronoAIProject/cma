use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    Router,
};
use layer2_core::{sandbox::*, AgentId, RuntimeCapabilities, RuntimeCapability};
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::{conversation::command_test_support, rest::agents_tests};

pub(super) const AGENT: &str = "agt_response_compat";
pub(super) const INPUT: &str = "Run printf response-compat-proof";

#[derive(Default)]
struct Runtime(Arc<AtomicUsize>);

#[async_trait]
impl AgentRuntimeRegistry for Runtime {
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        let dispatches = self.0.clone();
        Ok(AgentRuntimeConnection {
            capabilities: RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            input: AgentRuntimeInputSender::spawn(8, move |_| {
                dispatches.fetch_add(1, Ordering::SeqCst);
                async { Ok(()) }
            }),
        })
    }

    async fn fence(&self, _: &AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        Ok(())
    }
}

pub(super) struct Fixture {
    pub router: Router,
    pub recorder: Arc<DurableAgentUpdateRecorder>,
    pub assembly: AgentMessageAssembly,
    pub dispatches: Arc<AtomicUsize>,
}

pub(super) async fn fixture() -> Fixture {
    let runtime = Arc::new(Runtime::default());
    let (state, store) = agents_tests::conversation_fixture_with_registry(runtime.clone()).await;
    agents_tests::insert_agent(
        &store,
        agents_tests::seeded_agent(AGENT, AgentStatus::Ready, 2, None),
        vec![],
    )
    .await;
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    let state = command_test_support::compose(state, store.clone()).with_updates(recorder.clone());
    Fixture {
        router: command_test_support::router(state, store),
        recorder,
        assembly: AgentMessageAssembly::default(),
        dispatches: runtime.0.clone(),
    }
}

impl Fixture {
    pub async fn update(&mut self, value: Value) {
        record(&self.recorder, &mut self.assembly, value).await;
    }

    pub async fn create(&self) -> axum::response::Response {
        let response = self.router.clone().oneshot(request()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        response
    }

    pub async fn read(&self, path: &str) -> Value {
        let (status, body) = agents_tests::json_response(
            &self.router,
            agents_tests::request(Method::GET, path, "owner-a", None, None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }
}

pub(super) fn request() -> Request<Body> {
    agents_tests::request(
        Method::POST,
        "/api/v1/responses",
        "owner-a",
        Some("response-compat-key"),
        Some(json!({"conversation":AGENT, "input":INPUT, "stream":true})),
    )
}

async fn record(
    recorder: &DurableAgentUpdateRecorder,
    assembly: &mut AgentMessageAssembly,
    value: Value,
) {
    let update = if value == json!({"end_turn":true}) {
        AgentRuntimeUpdate::TurnEnded(AgentRuntimePromptResponse::new(
            AgentRuntimeTurnStopReason::EndTurn,
        ))
    } else {
        AgentRuntimeUpdate::Said(serde_json::from_value(value).unwrap())
    };
    recorder
        .record_update(
            &AgentId(AGENT.into()),
            1,
            assembly,
            update,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "loopback fixture for the typed SDK legacy Responses compatibility test"]
async fn legacy_responses_compat_server() {
    use axum::{
        extract::{Request, State},
        middleware::{self, Next},
        routing::{get, post},
        Json,
    };
    let fixture = fixture().await;
    let recorder = fixture.recorder;
    let assembly = Arc::new(tokio::sync::Mutex::new(fixture.assembly));
    let dispatches = fixture.dispatches;
    let (shutdown, stopped) = tokio::sync::oneshot::channel();
    let shutdown = Arc::new(tokio::sync::Mutex::new(Some(shutdown)));
    let router = fixture
        .router
        .route(
            "/__compat/update",
            post(move |Json(value): Json<Value>| {
                let recorder = recorder.clone();
                let assembly = assembly.clone();
                async move {
                    record(&recorder, &mut *assembly.lock().await, value).await;
                    StatusCode::NO_CONTENT
                }
            }),
        )
        .route(
            "/__compat/dispatches",
            get(move || {
                let dispatches = dispatches.clone();
                async move { Json(json!({"dispatches":dispatches.load(Ordering::SeqCst)})) }
            }),
        )
        .route(
            "/__compat/shutdown",
            post(move || {
                let shutdown = shutdown.clone();
                async move {
                    if let Some(shutdown) = shutdown.lock().await.take() {
                        let _ = shutdown.send(());
                    }
                    StatusCode::NO_CONTENT
                }
            }),
        )
        .layer(middleware::from_fn_with_state(
            (),
            |State(()): State<()>, mut request: Request, next: Next| async move {
                request.extensions_mut().insert(crate::AuthenticatedSubject(
                    layer2_core::SubjectId("owner-a".into()),
                ));
                request
                    .extensions_mut()
                    .insert(layer2_core::Authorization::new([
                        layer2_core::Capability::UseOwnSandboxes,
                    ]));
                next.run(request).await
            },
        ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let destination = std::env::var("CMA_RESPONSE_COMPAT_ADDRESS_FILE").expect("address file");
    std::fs::write(destination, address).unwrap();
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .await
        .unwrap();
}
