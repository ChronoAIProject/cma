use super::*;
use crate::auth::persistence::SecretValue;
use crate::sandbox::{
    credential::CredentialError,
    title_worker::{tests::fixtures as records, PlatformTitleEndpoint},
    CredentialMaterial,
};
use axum::{http::HeaderMap, routing::post, Json, Router};
use layer2_core::{
    budget::*,
    sandbox::store::{AgentStore as _, SandboxStore as _, SandboxTransactionStore as _},
    sandbox::*,
    *,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};
use tokio::sync::Semaphore;

const KEY: &str = "suggestion-scoped-fixture";
const MODEL: &str = "suggestion-model-fixture";

struct Budget {
    outcome: BudgetOutcome,
    entered: Semaphore,
    release: Semaphore,
    held: Mutex<Vec<RequestHold>>,
    settled: AtomicUsize,
}

#[derive(Clone, Copy)]
enum BudgetOutcome {
    Hold,
    NoHold,
    Refused,
}

#[async_trait::async_trait]
impl RequestBudget for Budget {
    async fn reserve_request(
        &self,
        scope: &RequestScope,
        request: &ModelRequest,
    ) -> Result<Option<RequestHold>, BudgetError> {
        let hold = RequestHold {
            scope: scope.clone(),
            request: request.clone(),
            input_tokens_bound: request.input_bytes + 64,
        };
        if matches!(self.outcome, BudgetOutcome::Hold) {
            self.held.lock().unwrap().push(hold.clone());
        }
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        match self.outcome {
            BudgetOutcome::Hold => Ok(Some(hold)),
            BudgetOutcome::NoHold => Ok(None),
            BudgetOutcome::Refused => Err(BudgetError::Unavailable),
        }
    }
    async fn settle_request(&self, hold: &RequestHold, _: RequestUsage) -> Result<(), BudgetError> {
        assert_eq!(hold.scope, self.held.lock().unwrap()[0].scope);
        self.settled.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct Credentials(Mutex<Option<CredentialMaterial>>);

#[async_trait::async_trait]
impl SandboxCredentialController for Credentials {
    async fn material(
        &self,
        _: &SandboxId,
        _: u64,
    ) -> Result<Option<CredentialMaterial>, CredentialError> {
        Ok(self.0.lock().unwrap().clone())
    }
    async fn issue(
        &self,
        _: &Sandbox,
        _: u64,
        _: SystemTime,
    ) -> Result<CredentialMaterial, CredentialError> {
        panic!("must not issue")
    }
    async fn custodied(&self, _: &SandboxId) -> Result<Vec<CredentialMaterial>, CredentialError> {
        panic!("must not enumerate")
    }
    async fn forget(&self, _: &SandboxId, _: u64) -> Result<(), CredentialError> {
        panic!("must not forget")
    }
    async fn revoke(&self, _: &Sandbox, _: &str) -> Result<(), CredentialError> {
        panic!("must not revoke")
    }
}

struct Reading(Mutex<SuggestionStatus>);
#[async_trait::async_trait]
impl SuggestionStore for Reading {
    async fn read_suggestions(
        &self,
        _: &SubjectId,
        _: &AgentId,
        _: agent_io::AnswerContext,
    ) -> Result<SuggestionReading, AgentReadError> {
        Ok(SuggestionReading {
            status: *self.0.lock().unwrap(),
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

struct Fixture {
    store: Arc<InMemorySandboxStore>,
    credentials: Arc<Credentials>,
    reading: Arc<Reading>,
    budget: Arc<Budget>,
    worker: Arc<SuggestionWorker>,
    work: SuggestionWork,
    requests: Arc<Mutex<Vec<serde_json::Value>>>,
    received: Arc<Semaphore>,
    reply: Arc<Semaphore>,
    server: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_budget(BudgetOutcome::Hold).await
    }

    async fn with_budget(outcome: BudgetOutcome) -> Self {
        let sandbox = records::sandbox();
        let agent = records::agent();
        let store = Arc::new(InMemorySandboxStore::default());
        store.create_sandbox(sandbox.clone()).await.unwrap();
        store.create_agent(agent.clone()).await.unwrap();
        let credentials = Arc::new(Credentials(Mutex::new(Some(CredentialMaterial {
            source: Default::default(),
            sandbox_id: sandbox.id.clone(),
            owner: sandbox.owner.clone(),
            authority: sandbox.credential.current.clone().unwrap(),
            value: SecretValue::new(KEY.into()),
            github_route: None,
        }))));
        let reading = Arc::new(Reading(Mutex::new(SuggestionStatus::Running)));
        let budget = Arc::new(Budget {
            outcome,
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            held: Mutex::new(vec![]),
            settled: AtomicUsize::new(0),
        });
        let requests = Arc::new(Mutex::new(vec![]));
        let capture = requests.clone();
        let received = Arc::new(Semaphore::new(0));
        let observed = received.clone();
        let reply = Arc::new(Semaphore::new(1));
        let release_reply = reply.clone();
        let app = Router::new().route(
            "/api/v1/proxy/s/chrono-llm/responses",
            post(
                move |headers: HeaderMap, Json(body): Json<serde_json::Value>| {
                    let capture = capture.clone();
                    let observed = observed.clone();
                    let release_reply = release_reply.clone();
                    async move {
                        assert_eq!(headers["authorization"], format!("Bearer {KEY}"));
                        assert_eq!(headers["idempotency-key"], "suggestion-dispatch-fixture");
                        assert!(!headers.contains_key("open-sandbox-api-key"));
                        assert_eq!(body["model"], MODEL);
                        capture.lock().unwrap().push(body);
                        observed.add_permits(1);
                        release_reply.acquire().await.unwrap().forget();
                        Json(serde_json::json!({
                            "status": "completed", "model": MODEL,
                            "usage": {"input_tokens": 7, "output_tokens": 3},
                            "output": [{"type": "message", "role": "assistant", "content": [
                                {"type": "output_text", "text": "{\"suggestions\":[]}"}
                            ]}]
                        }))
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Arc::new(PlatformTitleEndpoint {
            platform_base_url: format!("http://{}", listener.local_addr().unwrap()),
        });
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let policy = Arc::new(
            serde_json::from_value(serde_json::json!({
                "mode": "all", "accounts": [],
                "capabilities": ["suggested_followup_admission"]
            }))
            .unwrap(),
        );
        let mut worker = SuggestionWorker::with_dependencies(
            store.clone(),
            reading.clone(),
            credentials.clone(),
            endpoint,
            MODEL.into(),
            policy,
        );
        Arc::get_mut(&mut worker).unwrap().request_budget = Some(budget.clone());
        let work = SuggestionWork {
            id: "suggestion-dispatch-fixture".into(),
            owner: agent.owner.clone(),
            agent_id: agent.id.clone(),
            scope: agent_io::AnswerContext {
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
        };
        Self {
            store,
            credentials,
            reading,
            budget,
            worker,
            work,
            requests,
            received,
            reply,
            server,
        }
    }

    async fn start(
        &self,
    ) -> tokio::task::JoinHandle<Result<SuggestionGeneration, SuggestionFailure>> {
        let worker = self.worker.clone();
        let work = self.work.clone();
        let task = tokio::spawn(async move { worker.generate(&work).await });
        tokio::time::timeout(Duration::from_secs(3), self.budget.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        assert!(self.requests.lock().unwrap().is_empty());
        task
    }

    async fn complete(
        &self,
        task: tokio::task::JoinHandle<Result<SuggestionGeneration, SuggestionFailure>>,
    ) -> Result<SuggestionGeneration, SuggestionFailure> {
        self.budget.release.add_permits(1);
        Self::finish(task).await
    }

    async fn finish(
        mut task: tokio::task::JoinHandle<Result<SuggestionGeneration, SuggestionFailure>>,
    ) -> Result<SuggestionGeneration, SuggestionFailure> {
        let result = tokio::time::timeout(Duration::from_secs(3), &mut task).await;
        if result.is_err() {
            task.abort();
            let _ = task.await;
        }
        result.expect("suggestion completes").unwrap()
    }

    async fn change_authority(&self) {
        let mut sandbox = self
            .store
            .get_sandbox(&records::sandbox().id)
            .await
            .unwrap()
            .unwrap();
        let revision = sandbox.revision;
        sandbox
            .credential
            .current
            .as_mut()
            .unwrap()
            .provider_state_version += 1;
        sandbox.revision += 1;
        self.store.save_sandbox(revision, sandbox).await.unwrap();
    }

    async fn stop(mut self) {
        self.server.abort();
        assert!((&mut self.server)
            .await
            .is_err_and(|error| error.is_cancelled()));
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[tokio::test]
async fn authority_change_during_reservation_refuses_dispatch_and_retains_the_hold() {
    let f = Fixture::new().await;
    let task = f.start().await;
    f.change_authority().await;
    let result = f.complete(task).await;
    assert_eq!(
        f.requests.lock().unwrap().len(),
        0,
        "stale request reached the provider"
    );
    assert!(matches!(result, Err(SuggestionFailure::Unavailable)));
    assert_eq!(f.budget.held.lock().unwrap().len(), 1);
    assert_eq!(f.budget.settled.load(Ordering::SeqCst), 0);
    f.stop().await;
}

#[tokio::test]
async fn owner_scope_revision_during_reservation_blocks_the_old_credential() {
    let f = Fixture::new().await;
    let task = f.start().await;
    let mut sandbox = f
        .store
        .get_sandbox(&records::sandbox().id)
        .await
        .unwrap()
        .unwrap();
    let previous_revision = sandbox.revision;
    let old_authority = sandbox.credential.current.clone();
    let owner = sandbox.owner.clone();
    let at = now_rfc3339();
    let scope = sandbox
        .revise_live_service_access(
            &owner,
            &[
                ServiceId::new("chrono-llm"),
                ServiceId::new("synthetic-service"),
            ],
            at.clone(),
        )
        .unwrap();
    assert!(sandbox.needs_service_access_credential());
    assert_eq!(sandbox.credential.current, old_authority);
    assert_eq!(
        authority_digest(&records::agent(), &sandbox),
        f.work.authority
    );
    assert_eq!(
        f.credentials.0.lock().unwrap().as_ref().unwrap().value,
        SecretValue::new(KEY.into())
    );
    f.store
        .commit(store::SandboxTransaction::SaveSandboxAndAppend {
            command_revival: None,
            released_runtime: None,
            expected_revision: previous_revision,
            sandbox: Box::new(sandbox),
            event_at: at,
            event: SandboxEventKind::ServiceAccessRevised {
                scope: Box::new(scope),
            },
        })
        .await
        .unwrap();

    assert!(matches!(
        f.complete(task).await,
        Err(SuggestionFailure::Unavailable)
    ));
    assert!(f.requests.lock().unwrap().is_empty());
    assert_eq!(f.budget.held.lock().unwrap().len(), 1);
    assert_eq!(f.budget.settled.load(Ordering::SeqCst), 0);
    f.stop().await;
}

#[tokio::test]
async fn unchanged_authority_dispatches_once_and_settles_the_reserved_request() {
    let f = Fixture::new().await;
    let task = f.start().await;
    assert_eq!(
        f.complete(task).await.unwrap().output,
        "{\"suggestions\":[]}"
    );
    assert_eq!(f.requests.lock().unwrap().len(), 1);
    assert_eq!(f.budget.held.lock().unwrap().len(), 1);
    assert_eq!(f.budget.settled.load(Ordering::SeqCst), 1);
    f.stop().await;
}

#[tokio::test]
async fn missing_or_replaced_custody_during_reservation_refuses_dispatch() {
    for replacement in [None, Some("replacement-scoped-fixture")] {
        let f = Fixture::new().await;
        let task = f.start().await;
        if let Some(value) = replacement {
            f.credentials.0.lock().unwrap().as_mut().unwrap().value =
                SecretValue::new(value.into());
        } else {
            *f.credentials.0.lock().unwrap() = None;
        }
        let result = f.complete(task).await;
        assert_eq!(f.requests.lock().unwrap().len(), 0);
        assert!(matches!(result, Err(SuggestionFailure::Unavailable)));
        assert_eq!(f.budget.held.lock().unwrap().len(), 1);
        assert_eq!(f.budget.settled.load(Ordering::SeqCst), 0);
        f.stop().await;
    }
}

#[tokio::test]
async fn work_that_expires_during_reservation_is_not_dispatched() {
    let f = Fixture::new().await;
    let task = f.start().await;
    *f.reading.0.lock().unwrap() = SuggestionStatus::Expired;
    let result = f.complete(task).await;
    assert_eq!(f.requests.lock().unwrap().len(), 0);
    assert!(matches!(result, Err(SuggestionFailure::Unavailable)));
    assert_eq!(f.budget.held.lock().unwrap().len(), 1);
    assert_eq!(f.budget.settled.load(Ordering::SeqCst), 0);
    f.stop().await;
}

#[tokio::test]
async fn a_reservation_without_a_hold_still_rechecks_authority_before_dispatch() {
    let f = Fixture::with_budget(BudgetOutcome::NoHold).await;
    let task = f.start().await;
    f.change_authority().await;
    assert!(matches!(
        f.complete(task).await,
        Err(SuggestionFailure::Unavailable)
    ));
    assert!(f.requests.lock().unwrap().is_empty());
    assert!(f.budget.held.lock().unwrap().is_empty());
    assert_eq!(f.budget.settled.load(Ordering::SeqCst), 0);
    f.stop().await;
}

#[tokio::test]
async fn a_refused_reservation_never_dispatches_or_settles() {
    let f = Fixture::with_budget(BudgetOutcome::Refused).await;
    let task = f.start().await;
    assert!(matches!(
        f.complete(task).await,
        Err(SuggestionFailure::Unavailable)
    ));
    assert!(f.requests.lock().unwrap().is_empty());
    assert!(f.budget.held.lock().unwrap().is_empty());
    assert_eq!(f.budget.settled.load(Ordering::SeqCst), 0);
    f.stop().await;
}

#[tokio::test]
async fn authority_change_after_dispatch_keeps_the_original_usage_settlement() {
    let f = Fixture::new().await;
    f.reply.acquire().await.unwrap().forget();
    let task = f.start().await;
    f.budget.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(3), f.received.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    f.change_authority().await;
    f.reply.add_permits(1);
    assert!(Fixture::finish(task).await.is_ok());
    assert_eq!(f.requests.lock().unwrap().len(), 1);
    assert_eq!(f.budget.held.lock().unwrap().len(), 1);
    assert_eq!(f.budget.settled.load(Ordering::SeqCst), 1);
    f.stop().await;
}
