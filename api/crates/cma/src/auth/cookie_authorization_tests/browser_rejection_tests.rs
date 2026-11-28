//! Browser rejection is distinct from uncertain provider evidence at public routes.

use super::*;
use axum::body::{to_bytes, Body};
use axum::http::{Method, Request};
use layer1_public::rest::{agent_router, AgentRoutesState};
use layer2_core::profiles::InMemoryProfileStore;
use layer2_core::sandbox::store::{AgentStore, SandboxStore, WorkspaceStore, WorktreeStore};
use layer2_core::sandbox::{
    Agent, AgentOperationService, AgentReadService, AgentService, AgentStatus, InMemorySandboxStore,
};
use layer2_core::{OperationClaim, OperationReservation, OperationStore, RuntimeCapabilities};
use tower::ServiceExt as _;

pub(super) struct NoOperations;

#[async_trait::async_trait]
impl OperationStore for NoOperations {
    async fn reserve(&self, _: &OperationClaim) -> Result<OperationReservation, StoreError> {
        panic!("this request must not reserve work")
    }
    async fn complete(&self, _: &OperationClaim, _: Vec<u8>) -> Result<(), StoreError> {
        panic!("this request must not complete work")
    }
}

struct Browser {
    app: Router,
    headers: HeaderMap,
    resources: Arc<InMemorySandboxStore>,
    agent: Agent,
}

impl Browser {
    async fn new(fixture: &Fixture) -> Self {
        let resources = Arc::new(InMemorySandboxStore::default());
        let mut agent = resource_fixture::agent(
            "agent",
            "worktree",
            "workspace",
            "sandbox",
            "owner",
            AgentStatus::Ready,
        );
        agent.conversation.conversation_id = Some("retained-conversation".into());
        agent.conversation.runtime_generation = Some(1);
        agent.validate_conversation().unwrap();
        resources
            .create_sandbox(resource_fixture::sandbox("sandbox", "owner"))
            .await
            .unwrap();
        resources
            .create_workspace(resource_fixture::workspace("workspace", "sandbox", "owner"))
            .await
            .unwrap();
        resources
            .create_worktree(resource_fixture::worktree(
                "worktree",
                "workspace",
                "sandbox",
                "agent",
                "owner",
            ))
            .await
            .unwrap();
        resources.create_agent(agent.clone()).await.unwrap();
        let runtimes = Arc::new(crate::sandbox::LiveAgentRegistry::default());
        let routes = AgentRoutesState::new(
            layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap(),
            AgentService::new(
                resources.clone(),
                Arc::new(InMemoryProfileStore::default()),
                runtimes.clone(),
                RuntimeCapabilities::default(),
            ),
            AgentReadService::new(
                resources.clone(),
                Arc::new(crate::projections::UnavailableSandboxResources),
            ),
            AgentOperationService::new(resources.clone(), runtimes),
            Arc::new(NoOperations),
        );
        let app = crate::auth::router(fixture.auth.clone())
            .merge(agent_router(routes))
            .layer(axum::middleware::from_fn_with_state(
                fixture.auth.clone(),
                crate::auth::attach_subject,
            ));
        let login = fixture
            .auth
            .issue_browser_login(SubjectId::new("owner"))
            .await
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("cma_session={login}").parse().unwrap(),
        );
        Self {
            app,
            headers,
            resources,
            agent,
        }
    }

    async fn unchanged(&self, fixture: &Fixture, retained: &SubjectAuthState) {
        fixture.assert_unchanged(retained).await;
        assert_eq!(
            self.resources.get_agent(&self.agent.id).await.unwrap(),
            Some(self.agent.clone())
        );
        let login =
            crate::auth::session_routes::request_cookie(&self.headers, crate::auth::AUTH_COOKIE)
                .unwrap();
        assert!(fixture
            .store
            .browser_login(&SecretDigest::identify(
                crate::auth::AUTH_COOKIE_DOMAIN,
                login
            ))
            .await
            .unwrap()
            .is_some());
    }

    async fn request(&self, method: Method, path: &str) -> (StatusCode, Value) {
        request(&self.app, &self.headers, method, path).await
    }
}

async fn request(
    app: &Router,
    headers: &HeaderMap,
    method: Method,
    path: &str,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::from("{}"))
        .unwrap();
    *request.headers_mut() = headers.clone();
    request
        .headers_mut()
        .insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(!text.contains("must-not-leak"));
    assert!(!text.contains("fixture-token"));
    assert!(!text.contains("authorize again"));
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn routes() -> [(Method, &'static str); 3] {
    [
        (Method::GET, "/auth/session"),
        (Method::GET, "/api/v1/agents"),
        (Method::POST, "/api/v1/agents"),
    ]
}

fn assert_rejected(path: &str, result: (StatusCode, Value)) {
    assert_eq!(result.0, StatusCode::UNAUTHORIZED, "{path}: {}", result.1);
    if path == "/auth/session" {
        assert_eq!(
            result.1,
            json!({"authenticated": false, "silent_sign_in": false})
        );
    } else {
        assert_eq!(result.1["code"], "not_authenticated");
    }
}

#[tokio::test]
async fn explicit_browser_credential_refusal_is_401_without_destroying_retained_work_or_login() {
    for status in [StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN] {
        let fixture = Fixture::new().await;
        let retained = fixture.seed().await;
        let browser = Browser::new(&fixture).await;
        fixture.control.userinfo.write().await.status = status;
        fixture.control.userinfo.write().await.body =
            "OAuth authority changed or expired; authorize again".into();
        for (method, path) in routes() {
            assert_rejected(path, browser.request(method, path).await);
        }
        // The same verification remains retryable for retained/background work.
        assert_unavailable(fixture.request(retained.clone()).await);
        assert_eq!(fixture.control.exchange_requests.load(Ordering::SeqCst), 0);
        browser.unchanged(&fixture, &retained).await;
        *fixture.control.userinfo.write().await = Reply::json(json!({"sub": "owner"}));
        assert_eq!(
            browser.request(Method::GET, "/api/v1/agents").await.0,
            StatusCode::OK
        );
        browser.unchanged(&fixture, &retained).await;
    }
}

#[tokio::test]
async fn browser_provider_outages_preserve_classified_identity_errors_for_reads() {
    for (stage, status) in [
        ("discovery", StatusCode::SERVICE_UNAVAILABLE),
        ("userinfo", StatusCode::TOO_MANY_REQUESTS),
        ("userinfo", StatusCode::INTERNAL_SERVER_ERROR),
        ("userinfo", StatusCode::SERVICE_UNAVAILABLE),
    ] {
        let fixture = Fixture::new().await;
        let retained = fixture.seed().await;
        let browser = Browser::new(&fixture).await;
        let reply = match stage {
            "discovery" => &fixture.control.discovery,
            "userinfo" => &fixture.control.userinfo,
            _ => &fixture.control.introspection,
        };
        reply.write().await.status = status;
        for (method, path) in routes() {
            let (actual, body) = browser.request(method.clone(), path).await;
            assert_eq!(actual, StatusCode::SERVICE_UNAVAILABLE, "{stage}: {path}");
            let expected = if status == StatusCode::TOO_MANY_REQUESTS
                || (method == Method::GET && status == StatusCode::SERVICE_UNAVAILABLE)
            {
                "identity_unavailable"
            } else {
                "store_failed"
            };
            assert_eq!(body["code"], expected);
        }
        browser.unchanged(&fixture, &retained).await;
        reply.write().await.status = StatusCode::OK;
        assert_eq!(
            browser.request(Method::GET, "/api/v1/agents").await.0,
            StatusCode::OK
        );
    }
}

#[tokio::test]
async fn coalesced_browser_rejections_keep_their_type_and_next_request_verifies_again() {
    let fixture = Fixture::new().await;
    let retained = fixture.seed().await;
    let browser = Browser::new(&fixture).await;
    fixture.auth.metadata().await.unwrap();
    fixture.control.requests.store(0, Ordering::SeqCst);
    fixture.control.userinfo.write().await.status = StatusCode::UNAUTHORIZED;
    let gate = Arc::new(Gate::default());
    let introspection = Arc::new(Gate::default());
    fixture.control.userinfo.write().await.gate = Some(gate.clone());
    fixture.control.introspection.write().await.gate = Some(introspection.clone());
    let pending = (0..12)
        .map(|index| {
            let (app, headers) = (browser.app.clone(), browser.headers.clone());
            let (method, path) = routes()[index % 3].clone();
            tokio::spawn(async move { (path, request(&app, &headers, method, path).await) })
        })
        .collect::<Vec<_>>();
    let background = {
        let (auth, state) = (fixture.auth.clone(), retained.clone());
        tokio::spawn(async move { request_for_cookie(&auth, &state.subject, state.clone()).await })
    };
    concurrency_tests::participants(&fixture.auth, &retained, 13).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(gate.reached.notified(), introspection.reached.notified());
    })
    .await
    .unwrap();
    gate.release.notify_one();
    for pending in pending {
        let (path, result) = pending.await.unwrap();
        assert_rejected(path, result);
    }
    assert_unavailable(background.await.unwrap());
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst), 2);
    browser.unchanged(&fixture, &retained).await;
    fixture.control.userinfo.write().await.gate = None;
    fixture.control.userinfo.write().await.status = StatusCode::OK;
    fixture.control.introspection.write().await.gate = None;
    introspection.release.notify_waiters();
    assert_eq!(
        browser.request(Method::GET, "/api/v1/agents").await.0,
        StatusCode::OK
    );
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn rejection_of_a_replaced_cookie_generation_never_signs_out_the_current_login() {
    for change in ["generation", "sign_in", "access", "removed", "expired"] {
        let fixture = Fixture::new().await;
        let retained = fixture.seed().await;
        let browser = Browser::new(&fixture).await;
        fixture.control.userinfo.write().await.status = StatusCode::UNAUTHORIZED;
        let gate = Arc::new(Gate::default());
        fixture.control.userinfo.write().await.gate = Some(gate.clone());
        let pending = [Method::GET, Method::POST]
            .into_iter()
            .map(|method| {
                let (app, headers) = (browser.app.clone(), browser.headers.clone());
                tokio::spawn(async move { request(&app, &headers, method, "/api/v1/agents").await })
            })
            .collect::<Vec<_>>();
        concurrency_tests::participants(&fixture.auth, &retained, 2).await;
        tokio::time::timeout(Duration::from_secs(2), gate.reached.notified())
            .await
            .unwrap();
        let mut changed = retained.clone();
        changed.revision += 1;
        match change {
            "generation" => changed.token_generation += 1,
            "sign_in" => {
                changed = SubjectAuthState::new(
                    changed.subject,
                    changed.authorization,
                    changed.profile,
                    changed.tokens,
                )
            }
            "access" => {
                changed.tokens.as_mut().unwrap().access_token =
                    SecretValue::new("replaced-fixture-access".into())
            }
            "removed" => changed.tokens = None,
            "expired" => changed.tokens.as_mut().unwrap().expires_at = SystemTime::UNIX_EPOCH,
            _ => unreachable!(),
        }
        fixture.store.put_subject(changed.clone()).await.unwrap();
        gate.release.notify_one();
        for pending in pending {
            let (status, body) = pending.await.unwrap();
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{change}");
            assert_eq!(body["code"], "store_failed");
        }
        browser.unchanged(&fixture, &changed).await;
    }
}

#[tokio::test]
async fn browser_transport_failure_and_body_deadline_do_not_sign_out_or_mutate_credentials() {
    let fixture = Fixture::new().await;
    let retained = fixture.seed().await;
    let browser = Browser::new(&fixture).await;
    let metadata = fixture.auth.metadata().await.unwrap();
    let mut unavailable = metadata.clone();
    unavailable.userinfo_endpoint = Some("http://127.0.0.1:0/userinfo".into());
    *fixture.auth.fixture_metadata().write().await = Some(unavailable);
    for (method, path) in routes() {
        assert_eq!(
            browser.request(method, path).await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    *fixture.auth.fixture_metadata().write().await = Some(metadata);
    {
        let mut reply = fixture.control.userinfo.write().await;
        reply.body = "{".into();
        reply.stream_tail = Some((Duration::from_secs(2), Ok("}".into())));
    }
    let (status, body) = browser.request(Method::GET, "/api/v1/agents").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "store_failed");
    assert_eq!(body["retryable"], false);
    browser.unchanged(&fixture, &retained).await;
}

#[tokio::test]
async fn browser_due_refresh_still_rotates_in_place_before_successful_verification() {
    let fixture = Fixture::new().await;
    let mut retained = fixture.seed().await;
    retained.tokens.as_mut().unwrap().expires_at = SystemTime::now() + Duration::from_secs(30);
    fixture.store.put_subject(retained.clone()).await.unwrap();
    let browser = Browser::new(&fixture).await;
    assert_eq!(
        browser.request(Method::GET, "/api/v1/agents").await.0,
        StatusCode::OK
    );
    let rotated = fixture
        .store
        .subject(&retained.subject)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rotated.incarnation, retained.incarnation);
    assert_eq!(rotated.token_generation, retained.token_generation + 1);
    assert_eq!(
        rotated.tokens.as_ref().unwrap().access_token.expose(),
        "rotated-fixture-token"
    );
    assert_eq!(fixture.control.exchange_requests.load(Ordering::SeqCst), 1);
    browser.unchanged(&fixture, &rotated).await;
}
