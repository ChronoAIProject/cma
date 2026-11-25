use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};
use layer2_core::profiles::*;
use layer2_core::sandbox::store::{AgentStore as _, PageRequest, SandboxStore as _};
use layer2_core::sandbox::*;
use layer2_core::*;
use tokio::sync::Semaphore;

use super::*;
use crate::auth::persistence::SecretValue;
use crate::sandbox::credential::CredentialError;

const NOW: &str = "2026-09-05T00:00:00Z";
const KEY: &str = "admitted-title-key";
const MODEL: &str = "deployment-title-model";

struct Budget(std::sync::atomic::AtomicBool);
#[async_trait::async_trait]
impl crate::sandbox::title_budget::TitleBudget for Budget {
    async fn admit(&self) -> Result<bool, AgentTitleServiceError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}
struct Credentials(Mutex<Option<CredentialMaterial>>, AtomicUsize);

#[async_trait::async_trait]
impl SandboxCredentialController for Credentials {
    async fn material(
        &self,
        id: &SandboxId,
        generation: u64,
    ) -> Result<Option<CredentialMaterial>, CredentialError> {
        self.1.fetch_add(1, Ordering::SeqCst);
        assert_eq!(id.as_str(), "sbx_title");
        assert_eq!(generation, 3);
        Ok(self.0.lock().unwrap().clone())
    }
    async fn issue(
        &self,
        _: &Sandbox,
        _: u64,
        _: SystemTime,
    ) -> Result<CredentialMaterial, CredentialError> {
        panic!("title must not issue credentials")
    }
    async fn custodied(&self, _: &SandboxId) -> Result<Vec<CredentialMaterial>, CredentialError> {
        panic!("title must not enumerate credentials")
    }
    async fn forget(&self, _: &SandboxId, _: u64) -> Result<(), CredentialError> {
        panic!("title must not forget credentials")
    }
    async fn revoke(&self, _: &Sandbox, _: &str) -> Result<(), CredentialError> {
        panic!("title must not revoke credentials")
    }
}

struct Runtime {
    endpoint: ProviderEndpoint,
    calls: AtomicUsize,
    gate: Option<Arc<Semaphore>>,
}

#[async_trait::async_trait]
impl TitleRuntimeEndpoint for Runtime {
    async fn endpoint(&self, sandbox: &Sandbox) -> Option<TitleProxy> {
        assert_eq!(sandbox.id.as_str(), "sbx_title");
        assert_eq!(
            sandbox.runtime.as_ref().unwrap().runtime_id,
            "runtime-title"
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = &self.gate {
            gate.acquire().await.unwrap().forget();
        }
        Some(TitleProxy {
            endpoint: self.endpoint.clone(),
            authorization: Some(SecretValue::new("title-routing-key".into())),
        })
    }
}

#[derive(Default)]
struct Capture {
    requests: Mutex<Vec<(String, serde_json::Value)>>,
    gate: Option<Arc<Semaphore>>,
    malformed: bool,
}

async fn respond(
    State(capture): State<Arc<Capture>>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    assert_eq!(headers["open-sandbox-api-key"], "title-routing-key");
    capture
        .requests
        .lock()
        .unwrap()
        .push((headers["authorization"].to_str().unwrap().into(), body));
    if let Some(gate) = &capture.gate {
        gate.acquire().await.unwrap().forget();
    }
    Json(
        serde_json::json!({"output_text": if capture.malformed { "***" } else { "### Durable Scoped Title\nignored" }}),
    )
}

struct Fixture {
    id: AgentId,
    store: Arc<InMemorySandboxStore>,
    titles: Arc<InMemoryAgentTitleStore>,
    service: Arc<AgentTitleService>,
    worker: Arc<AgentTitleWorker>,
    budget: Arc<Budget>,
    credentials: Arc<Credentials>,
    runtime: Arc<Runtime>,
    capture: Arc<Capture>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn new(capture: Capture, endpoint_gate: Option<Arc<Semaphore>>) -> Self {
        Self::with_records(sandbox(), agent(), capture, endpoint_gate).await
    }

    async fn with_records(
        sandbox: Sandbox,
        agent: Agent,
        capture: Capture,
        endpoint_gate: Option<Arc<Semaphore>>,
    ) -> Self {
        let id = agent.id.clone();
        let store = Arc::new(InMemorySandboxStore::default());
        store.create_sandbox(sandbox.clone()).await.unwrap();
        store.create_agent(agent).await.unwrap();
        let titles = Arc::new(InMemoryAgentTitleStore::default());
        titles.register_agent(id.clone()).unwrap();
        let service = AgentTitleService::new(titles.clone(), "title-worker").unwrap();
        let credentials = Arc::new(Credentials(
            Mutex::new(Some(CredentialMaterial {
                source: Default::default(),
                github_route: None,
                sandbox_id: sandbox.id,
                owner: sandbox.owner,
                authority: sandbox.credential.current.unwrap(),
                value: SecretValue::new(KEY.into()),
            })),
            AtomicUsize::new(0),
        ));
        let capture = Arc::new(capture);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = ProviderEndpoint {
            endpoint: format!("http://{}/proxy/9091", listener.local_addr().unwrap()),
        };
        let router = Router::new()
            .route("/proxy/9091/v1/responses", post(respond))
            .with_state(capture.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let runtime = Arc::new(Runtime {
            endpoint,
            calls: AtomicUsize::new(0),
            gate: endpoint_gate,
        });
        let budget = Arc::new(Budget(std::sync::atomic::AtomicBool::new(true)));
        let worker = AgentTitleWorker::new(
            service.clone(),
            store.clone(),
            credentials.clone(),
            runtime.clone(),
            MODEL.into(),
            budget.clone(),
        );
        Self {
            id,
            store,
            titles,
            service,
            worker,
            budget,
            credentials,
            runtime,
            capture,
            server,
        }
    }

    fn turn(&self, sequence: u64, owner: &str) {
        for kind in [
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: sequence,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: owner.into(),
            },
            AgentLogKind::AgentMessage {
                content: format!("answer {sequence}"),
            },
            AgentLogKind::TurnCompleted {
                turn_sequence: sequence,
            },
        ] {
            self.titles.append_agent_log(&self.id, NOW, kind).unwrap();
        }
    }
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !condition() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn completed_turn_reaches_scoped_runtime_proxy_once_and_survives_restart() {
    let fixture = Fixture::new(Capture::default(), None).await;
    assert_eq!(
        fixture.worker.run_one(&fixture.id).await.unwrap(),
        AgentTitleOutcome::Skipped
    );
    assert_eq!(fixture.runtime.calls.load(Ordering::SeqCst), 0);
    fixture.turn(7, "First durable owner message");
    fixture.turn(8, "Later owner message");
    assert_eq!(
        fixture
            .worker
            .candidates(&PageRequest::default())
            .await
            .unwrap()
            .items,
        vec![fixture.id.clone()]
    );
    assert_eq!(
        fixture.worker.run_one(&fixture.id).await.unwrap(),
        AgentTitleOutcome::Generated
    );
    let stored = fixture.service.title(&fixture.id).await.unwrap().unwrap();
    assert_eq!(stored.title.as_str(), "Durable Scoped Title");
    {
        let requests = fixture.capture.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, format!("Bearer {KEY}"));
        assert_eq!(requests[0].1["model"], MODEL);
        assert_eq!(
            requests[0].1["input"][0]["content"],
            "First durable owner message"
        );
        assert_eq!(requests[0].1["input"][1]["content"], "answer 7");
    }
    let restarted = AgentTitleWorker::new(
        AgentTitleService::new(fixture.titles.clone(), "restarted").unwrap(),
        fixture.store.clone(),
        fixture.credentials.clone(),
        fixture.runtime.clone(),
        MODEL.into(),
        fixture.budget.clone(),
    );
    assert_eq!(
        restarted.run_one(&fixture.id).await.unwrap(),
        AgentTitleOutcome::Skipped
    );
    assert_eq!(fixture.runtime.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn transient_missing_custody_does_not_consume_the_title_claim() {
    let fixture = Fixture::new(Capture::default(), None).await;
    fixture.turn(1, "Deferred title");
    let material = fixture.credentials.0.lock().unwrap().take().unwrap();
    assert_eq!(
        fixture.worker.run_one(&fixture.id).await.unwrap(),
        AgentTitleOutcome::Skipped
    );
    assert!(fixture.service.title(&fixture.id).await.unwrap().is_none());
    *fixture.credentials.0.lock().unwrap() = Some(material);
    assert_eq!(
        fixture.worker.run_one(&fixture.id).await.unwrap(),
        AgentTitleOutcome::Generated
    );
}

#[tokio::test]
async fn mismatched_custody_cannot_receive_first_turn_content() {
    for change in 0..7 {
        let fixture = Fixture::new(Capture::default(), None).await;
        fixture.turn(1, "Private title input");
        {
            let mut material = fixture.credentials.0.lock().unwrap();
            let material = material.as_mut().unwrap();
            match change {
                0 => material.owner = SubjectId::new("other"),
                1 => material.sandbox_id = SandboxId("other".into()),
                2 => material.authority.generation += 1,
                3 => material.authority.provider_key_id = "other".into(),
                4 => material.authority.provider_state_version += 1,
                5 => material.authority.grant_fingerprint = "c".repeat(64),
                _ => material.value = SecretValue::new(" ".into()),
            }
        }
        assert_eq!(
            fixture.worker.run_one(&fixture.id).await.unwrap(),
            AgentTitleOutcome::Skipped
        );
        assert_eq!(fixture.runtime.calls.load(Ordering::SeqCst), 0);
        assert!(fixture.capture.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn unavailable_or_mismatched_runtime_authority_is_deferred_without_claiming() {
    for change in 0..6 {
        let mut sandbox = sandbox();
        let mut agent = agent();
        match change {
            0 => sandbox.status = SandboxStatus::Paused,
            1 => sandbox.runtime = None,
            2 => sandbox.credential.delivered = false,
            3 => agent.owner = SubjectId::new("other"),
            4 => agent.conversation.runtime_generation = Some(9),
            _ => sandbox.credential.current.as_mut().unwrap().expires_at = NOW.into(),
        }
        let fixture = Fixture::with_records(sandbox, agent, Capture::default(), None).await;
        fixture.turn(1, "Pending authority title");
        assert_eq!(
            fixture.worker.run_one(&fixture.id).await.unwrap(),
            AgentTitleOutcome::Skipped
        );
        assert!(fixture.service.title(&fixture.id).await.unwrap().is_none());
        assert_eq!(fixture.runtime.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn own_account_runtime_generates_with_scoped_service_custody() {
    async fn scoped(
        headers: HeaderMap,
        Json(body): Json<serde_json::Value>,
    ) -> Json<serde_json::Value> {
        assert_eq!(headers["authorization"], format!("Bearer {KEY}"));
        assert!(!headers.contains_key("open-sandbox-api-key"));
        assert_eq!(body["model"], MODEL);
        assert_eq!(body["input"][0]["content"], "Own account title");
        Json(serde_json::json!({"output_text":"Own credential task title"}))
    }
    let mut sandbox = sandbox();
    sandbox.profile.content.model_access = ModelAccess::OwnCredentials;
    let f = Fixture::with_records(sandbox, agent(), Capture::default(), None).await;
    f.turn(1, "Own account title");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = Arc::new(PlatformTitleEndpoint {
        platform_base_url: format!("http://{}", listener.local_addr().unwrap()),
    });
    let router = Router::new().route("/api/v1/proxy/s/chrono-llm/responses", post(scoped));
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let worker = AgentTitleWorker::new(
        f.service.clone(),
        f.store.clone(),
        f.credentials.clone(),
        endpoint,
        MODEL.into(),
        f.budget.clone(),
    );
    assert_eq!(
        worker.run_one(&f.id).await.unwrap(),
        AgentTitleOutcome::Generated
    );
    assert_eq!(
        f.service
            .title(&f.id)
            .await
            .unwrap()
            .unwrap()
            .title
            .as_str(),
        "Own credential task title"
    );
    assert_eq!(f.runtime.calls.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn authority_drift_during_endpoint_lookup_prevents_http_disclosure() {
    let gate = Arc::new(Semaphore::new(0));
    let fixture = Fixture::new(Capture::default(), Some(gate.clone())).await;
    fixture.turn(1, "Safe fallback after drift");
    let worker = fixture.worker.clone();
    let id = fixture.id.clone();
    let task = tokio::spawn(async move { worker.run_one(&id).await });
    wait_until(|| fixture.runtime.calls.load(Ordering::SeqCst) == 1).await;
    fixture
        .credentials
        .0
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .authority
        .provider_state_version += 1;
    gate.add_permits(1);
    assert!(matches!(
        task.await.unwrap(),
        Err(AgentTitleServiceError::GenerationUnavailable)
    ));
    assert!(fixture.capture.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn user_title_wins_an_inflight_generated_result() {
    let gate = Arc::new(Semaphore::new(0));
    let fixture = Fixture::new(
        Capture {
            gate: Some(gate.clone()),
            ..Default::default()
        },
        None,
    )
    .await;
    fixture.turn(1, "User title race");
    let worker = fixture.worker.clone();
    let id = fixture.id.clone();
    let task = tokio::spawn(async move { worker.run_one(&id).await });
    wait_until(|| fixture.capture.requests.lock().unwrap().len() == 1).await;
    fixture
        .service
        .set_user_title(&fixture.id, &AgentTitle::new("Chosen By Owner").unwrap())
        .await
        .unwrap();
    gate.add_permits(1);
    assert_eq!(task.await.unwrap().unwrap(), AgentTitleOutcome::Skipped);
    assert_eq!(
        fixture
            .service
            .title(&fixture.id)
            .await
            .unwrap()
            .unwrap()
            .source,
        AgentTitleSource::User
    );
    assert_eq!(fixture.titles.title_event_count(&fixture.id).unwrap(), 1);
}

#[tokio::test]
async fn credential_drift_during_http_refuses_the_stale_generated_result() {
    let gate = Arc::new(Semaphore::new(0));
    let fixture = Fixture::new(
        Capture {
            gate: Some(gate.clone()),
            ..Default::default()
        },
        None,
    )
    .await;
    fixture.turn(1, "Current authority fallback");
    let worker = fixture.worker.clone();
    let id = fixture.id.clone();
    let task = tokio::spawn(async move { worker.run_one(&id).await });
    wait_until(|| fixture.capture.requests.lock().unwrap().len() == 1).await;
    fixture
        .credentials
        .0
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .authority
        .provider_state_version += 1;
    gate.add_permits(1);
    assert!(matches!(
        task.await.unwrap(),
        Err(AgentTitleServiceError::GenerationUnavailable)
    ));
    assert!(fixture.service.title(&fixture.id).await.unwrap().is_none());
    assert_eq!(fixture.capture.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn failed_attempts_defer_and_expired_claim_allows_generation() {
    let fixture = Fixture::new(
        Capture {
            malformed: true,
            ..Default::default()
        },
        None,
    )
    .await;
    let owner = "First message ".repeat(10);
    fixture.turn(1, &owner);
    assert!(matches!(
        fixture.worker.run_one(&fixture.id).await,
        Err(AgentTitleServiceError::GenerationUnavailable)
    ));
    assert_eq!(fixture.capture.requests.lock().unwrap().len(), 1);
    assert!(fixture.service.title(&fixture.id).await.unwrap().is_none());
    let crashed = Fixture::new(Capture::default(), None).await;
    crashed.turn(1, "Restart fallback");
    crashed
        .titles
        .acquire_agent_title_claim(
            &AgentTitleClaimRequest::new(crashed.id.clone(), "crashed"),
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crashed.worker.run_one(&crashed.id).await.unwrap(),
        AgentTitleOutcome::Generated
    );
    assert_eq!(crashed.runtime.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn snapshot_identity_binds_owner_lineage_runtime_and_credential() {
    let expected = TitleAuthority {
        agent: agent(),
        sandbox: sandbox(),
    };
    for change in 0..9 {
        let mut other = TitleAuthority {
            agent: agent(),
            sandbox: sandbox(),
        };
        match change {
            0 => other.agent.owner = SubjectId::new("other"),
            1 => other.agent.sandbox_id = SandboxId("other".into()),
            2 => other.agent.worktree_id = WorktreeId("other".into()),
            3 => other.agent.workspace_id = WorkspaceId("other".into()),
            4 => other.agent.conversation.runtime_generation = Some(9),
            5 => other.sandbox.runtime.as_mut().unwrap().runtime_id = "other".into(),
            6 => other.sandbox.runtime.as_mut().unwrap().provision_generation = Some(9),
            7 => {
                other
                    .sandbox
                    .credential
                    .current
                    .as_mut()
                    .unwrap()
                    .generation += 1
            }
            _ => other.sandbox.credential.reaches.clear(),
        }
        assert!(!expected.matches(&other));
    }
}

#[path = "title_worker_tests/fixtures.rs"]
pub(in crate::sandbox) mod fixtures;
use fixtures::{agent, sandbox};

#[tokio::test]
async fn native_title_endpoint_uses_the_scoped_public_model_service() {
    let endpoint = PlatformTitleEndpoint {
        platform_base_url: "https://identity.example".into(),
    };
    let mut sandbox = sandbox();
    sandbox.runtime.as_mut().unwrap().runtime_id = "agent-sandbox:retained-home".into();
    sandbox.credential.reaches = vec![ServiceId::new("chrono-llm-public")];
    let proxy = endpoint.endpoint(&sandbox).await.unwrap();
    assert_eq!(
        proxy.endpoint.endpoint,
        "https://identity.example/api/v1/proxy/s/chrono-llm-public"
    );
    assert!(proxy.authorization.is_none());
    sandbox.credential.reaches.clear();
    assert!(endpoint.endpoint(&sandbox).await.is_none());
}

#[tokio::test]
async fn exhausted_shared_generation_budget_never_calls_the_model_or_spends_a_title_attempt() {
    let f = Fixture::new(Capture::default(), None).await;
    f.turn(1, "Budgeted title");
    f.budget.0.store(false, Ordering::SeqCst);
    assert_eq!(
        f.worker.run_one(&f.id).await.unwrap(),
        AgentTitleOutcome::Skipped
    );
    assert_eq!(f.capture.requests.lock().unwrap().len(), 0);
    f.budget.0.store(true, Ordering::SeqCst);
    assert_eq!(
        f.worker.run_one(&f.id).await.unwrap(),
        AgentTitleOutcome::Generated
    );
    assert_eq!(f.capture.requests.lock().unwrap().len(), 1);
}

#[path = "title_worker_tests/suggestions.rs"]
mod suggestion_tests;

#[path = "title_worker_tests/auxiliary_routes.rs"]
mod auxiliary_routes;

#[tokio::test]
async fn missing_scoped_model_access_settles_fallback_without_network_retries() {
    let mut sandbox = sandbox();
    sandbox.credential.reaches = vec![ServiceId::new("github")];
    let f = Fixture::with_records(sandbox, agent(), Capture::default(), None).await;
    f.turn(1, "An unscoped task");
    let worker = AgentTitleWorker::new(
        f.service.clone(),
        f.store.clone(),
        f.credentials.clone(),
        Arc::new(PlatformTitleEndpoint {
            platform_base_url: "http://127.0.0.1:1".into(),
        }),
        MODEL.into(),
        f.budget.clone(),
    );
    assert_eq!(
        worker.run_one(&f.id).await.unwrap(),
        AgentTitleOutcome::Fallback
    );
    assert_eq!(
        worker.run_one(&f.id).await.unwrap(),
        AgentTitleOutcome::Skipped
    );
    assert!(f
        .service
        .candidates(&PageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
    assert!(f.capture.requests.lock().unwrap().is_empty());
}
