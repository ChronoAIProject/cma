use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use layer2_core::Capability;
use serde_json::{json, Value};
use tokio::sync::{Notify, RwLock};

use super::*;
use crate::auth::persistence::{AuthStore, DurableSubjectTokens, MemoryAuthStore, SecretValue};
use crate::auth::{profile::Profile, test_resources as resource_fixture, AuthMode};

#[path = "cookie_authorization_tests/actor_link_tests.rs"]
mod actor_link_tests;
#[path = "cookie_authorization_tests/bearer_availability_tests.rs"]
mod bearer_availability_tests;
#[path = "cookie_authorization_tests/bots_tests.rs"]
mod bots_tests;
#[path = "cookie_authorization_tests/browser_rejection_tests.rs"]
mod browser_rejection_tests;
#[path = "cookie_authorization_tests/concurrency_tests.rs"]
mod concurrency_tests;
#[path = "cookie_authorization_tests/deferred_command_tests.rs"]
mod deferred_command_tests;
#[path = "cookie_authorization_tests/personal_owner_tests.rs"]
mod personal_owner_tests;
#[path = "cookie_authorization_tests/provider_overlap_tests.rs"]
mod provider_overlap_tests;
#[path = "cookie_authorization_tests/resource_command_tests.rs"]
mod resource_command_tests;
#[path = "cookie_authorization_tests/session_routes_tests.rs"]
mod session_routes_tests;

#[derive(Default)]
struct Gate {
    reached: Notify,
    release: Notify,
}

#[derive(Clone)]
struct Reply {
    status: StatusCode,
    body: String,
    delay: Duration,
    gate: Option<Arc<Gate>>,
}

impl Reply {
    fn json(body: Value) -> Self {
        Self {
            status: StatusCode::OK,
            body: body.to_string(),
            delay: Duration::ZERO,
            gate: None,
        }
    }
}

struct ProviderControl {
    discovery: RwLock<Reply>,
    userinfo: RwLock<Reply>,
    introspection: RwLock<Reply>,
    organization: RwLock<Reply>,
    exchange: RwLock<Reply>,
    organization_token: RwLock<String>,
    organization_requests: AtomicUsize,
    exchange_requests: AtomicUsize,
    requests: AtomicUsize,
}

async fn respond(control: &ProviderControl, reply: &RwLock<Reply>) -> Response {
    control.requests.fetch_add(1, Ordering::SeqCst);
    let reply = reply.read().await.clone();
    if let Some(gate) = reply.gate {
        gate.reached.notify_one();
        gate.release.notified().await;
    }
    tokio::time::sleep(reply.delay).await;
    (
        reply.status,
        [(header::CONTENT_TYPE, "application/json")],
        reply.body,
    )
        .into_response()
}

async fn discovery(State(control): State<Arc<ProviderControl>>) -> Response {
    respond(&control, &control.discovery).await
}

async fn userinfo(State(control): State<Arc<ProviderControl>>) -> Response {
    respond(&control, &control.userinfo).await
}

async fn introspection(State(control): State<Arc<ProviderControl>>) -> Response {
    respond(&control, &control.introspection).await
}

async fn organization(State(control): State<Arc<ProviderControl>>, headers: HeaderMap) -> Response {
    assert_eq!(
        headers[header::AUTHORIZATION],
        format!("Bearer {}", control.organization_token.read().await)
    );
    control.organization_requests.fetch_add(1, Ordering::SeqCst);
    respond(&control, &control.organization).await
}

async fn exchange(State(control): State<Arc<ProviderControl>>) -> Response {
    control.exchange_requests.fetch_add(1, Ordering::SeqCst);
    respond(&control, &control.exchange).await
}

struct Fixture {
    auth: Arc<AuthState>,
    store: Arc<MemoryAuthStore>,
    control: Arc<ProviderControl>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let control = Arc::new(ProviderControl {
            discovery: RwLock::new(Reply::json(json!({
                "authorization_endpoint": format!("{base}/authorize"),
                "token_endpoint": format!("{base}/token"),
                "userinfo_endpoint": format!("{base}/userinfo"),
                "introspection_endpoint": format!("{base}/introspect"),
            }))),
            userinfo: RwLock::new(Reply::json(json!({
                "sub": "owner", "name": "Live Owner", "permissions": ["cma:*"],
            }))),
            introspection: RwLock::new(Reply::json(json!({
                "active": true, "sub": "owner", "permissions": ["cma:sandbox:use"],
            }))),
            organization: RwLock::new(Reply::json(json!({"id":"org", "your_role":"admin"}))),
            exchange: RwLock::new(Reply::json(json!({
                "access_token":"rotated-fixture-token", "refresh_token":"rotated-fixture-refresh",
                "expires_in":3600
            }))),
            organization_token: RwLock::new("must-not-leak-access-token".into()),
            organization_requests: AtomicUsize::new(0),
            exchange_requests: AtomicUsize::new(0),
            requests: AtomicUsize::new(0),
        });
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/userinfo", get(userinfo))
            .route("/introspect", post(introspection))
            .route("/token", post(exchange))
            .route("/api/v1/orgs/org/authorization", get(organization))
            .with_state(control.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let store = Arc::new(MemoryAuthStore::default());
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: base,
                client_id: "cookie-client".into(),
                client_secret: "must-not-leak-client-secret".into(),
                public_url: "https://cma.example.test".into(),
            },
            None,
            store.clone(),
        );
        Self {
            auth,
            store,
            control,
            server,
        }
    }

    async fn seed(&self) -> SubjectAuthState {
        let state = SubjectAuthState::new(
            SubjectId::new("owner"),
            crate::permissions::capabilities_from_permissions(["cma:*"]),
            Profile {
                display_name: Some("Stored Owner".into()),
                ..Profile::default()
            },
            Some(DurableSubjectTokens {
                access_token: SecretValue::new("must-not-leak-access-token".into()),
                refresh_token: Some(SecretValue::new("must-not-leak-refresh-token".into())),
                expires_at: SystemTime::now() + Duration::from_secs(3600),
            }),
        );
        self.store.put_subject(state.clone()).await.unwrap();
        state
    }

    async fn request(&self, state: SubjectAuthState) -> Result<RequestAuth, StoreError> {
        request_for_cookie(&self.auth, &SubjectId::new("owner"), state).await
    }

    async fn blocked_request(
        &self,
        state: SubjectAuthState,
    ) -> (
        Arc<Gate>,
        tokio::task::JoinHandle<Result<RequestAuth, StoreError>>,
    ) {
        let gate = Arc::new(Gate::default());
        self.control.introspection.write().await.gate = Some(gate.clone());
        let auth = self.auth.clone();
        let pending = tokio::spawn(async move {
            request_for_cookie(&auth, &SubjectId::new("owner"), state).await
        });
        tokio::time::timeout(Duration::from_secs(2), gate.reached.notified())
            .await
            .expect("provider verification should reach introspection");
        (gate, pending)
    }

    async fn assert_unchanged(&self, expected: &SubjectAuthState) {
        let actual = self
            .store
            .subject(&expected.subject)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(actual.incarnation, expected.incarnation);
        assert_eq!(actual.revision, expected.revision);
        assert_eq!(actual.token_generation, expected.token_generation);
        assert_eq!(actual.refresh_lease, expected.refresh_lease);
        assert_eq!(actual.tokens, expected.tokens);
        assert_eq!(actual.authorization, expected.authorization);
        assert_eq!(actual.profile, expected.profile);
    }
}

fn assert_unavailable(result: Result<RequestAuth, StoreError>) {
    match result {
        Err(StoreError::Unavailable(message)) => {
            assert_eq!(message, "cookie authorization could not be verified");
        }
        Err(error) => panic!("expected sanitized unavailable, got {error:?}"),
        Ok(_) => panic!("unverifiable provider evidence must not authenticate the cookie"),
    }
}

#[tokio::test]
async fn cookie_uses_live_reduced_permissions_on_every_request_without_writes() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    let first = fixture.request(state.clone()).await.unwrap();
    assert_eq!(
        first.authorization,
        Authorization::new([Capability::UseOwnSandboxes])
    );
    assert_eq!(first.profile.display_name.as_deref(), Some("Live Owner"));
    assert!(first.delegated_credential.is_some());
    fixture.assert_unchanged(&state).await;

    *fixture.control.introspection.write().await =
        Reply::json(json!({"active": true, "permissions": []}));
    let second = fixture.request(state.clone()).await.unwrap();
    assert_eq!(second.authorization, Authorization::none());
    assert!(second.delegated_credential.is_some());
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst), 5);
    fixture.assert_unchanged(&state).await;
}

#[tokio::test]
async fn inactive_cookie_retains_identity_but_no_grants_or_delegated_credential() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    *fixture.control.introspection.write().await = Reply::json(json!({"active": false}));
    let request = fixture.request(state.clone()).await.unwrap();
    assert_eq!(request.subject, state.subject);
    assert_eq!(request.authorization, Authorization::none());
    assert!(request.delegated_credential.is_none());
    fixture.assert_unchanged(&state).await;

    let metadata = fixture.auth.metadata().await.unwrap();
    assert!(
        identity::resolve_subject(&fixture.auth, &metadata, "access")
            .await
            .is_err()
    );
    assert!(identity::request_for_bearer(&fixture.auth, "access".into())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn relay_owner_projection_cannot_establish_ordinary_bearer_or_login_authority() {
    use base64::Engine as _;
    let fixture = Fixture::new().await;
    let metadata = fixture.auth.metadata().await.unwrap();
    let before = fixture.control.requests.load(Ordering::SeqCst);
    for channel in [
        json!({"relay":true, "sub":"owner", "permissions":["cma:*"]}),
        json!({"token_type":"relay_callback"}),
        json!({"token_type":"relay_reply"}),
    ] {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&channel).unwrap());
        let token = format!("header.{payload}.signature");
        assert!(identity::request_for_bearer(&fixture.auth, token.clone())
            .await
            .unwrap()
            .is_none());
        assert!(identity::resolve_subject(&fixture.auth, &metadata, &token)
            .await
            .is_err());
        assert_eq!(fixture.control.requests.load(Ordering::SeqCst), before);
    }
    let ordinary = identity::request_for_bearer(&fixture.auth, "access".into())
        .await
        .unwrap()
        .unwrap();
    assert!(ordinary.authorization.allows(Capability::UseOwnSandboxes));
    assert_eq!(ordinary.subject, layer2_core::SubjectId::new("owner"));
    assert!(fixture.control.requests.load(Ordering::SeqCst) > before);
}

#[tokio::test]
async fn unusable_durable_access_never_reuses_stored_authority_or_calls_provider() {
    let fixture = Fixture::new().await;
    let original = fixture.seed().await;
    for case in 0..3 {
        let mut state = original.clone();
        match case {
            0 => state.tokens = None,
            1 => state.tokens.as_mut().unwrap().expires_at = SystemTime::UNIX_EPOCH,
            _ => state.tokens.as_mut().unwrap().access_token = SecretValue::new("  ".into()),
        }
        fixture.store.put_subject(state.clone()).await.unwrap();
        let request = fixture.request(state.clone()).await.unwrap();
        assert_eq!(request.subject, state.subject);
        assert_eq!(request.authorization, Authorization::none());
        assert_eq!(request.profile, state.profile);
        assert!(request.delegated_credential.is_none());
        fixture.assert_unchanged(&state).await;
    }
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn malformed_or_missing_active_introspection_never_uses_partial_or_userinfo_grants() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    for body in [
        json!({"permissions": ["cma:*"]}),
        json!({"active": "true", "permissions": ["cma:*"]}),
        json!({"active": true}),
        json!({"active": true, "permissions": "cma:*"}),
        json!({"active": true, "permissions": ["cma:*", 17]}),
        json!({"active": true, "sub": null, "permissions": ["cma:*"]}),
    ] {
        *fixture.control.introspection.write().await = Reply::json(body);
        assert_unavailable(fixture.request(state.clone()).await);
        fixture.assert_unchanged(&state).await;
    }
    fixture.control.introspection.write().await.body = "{".into();
    assert_unavailable(fixture.request(state.clone()).await);
    fixture.assert_unchanged(&state).await;
}

#[tokio::test]
async fn userinfo_and_introspection_subjects_must_match_the_exact_cookie_owner() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    for subject in ["other", " owner "] {
        *fixture.control.userinfo.write().await = Reply::json(json!({"sub": subject}));
        *fixture.control.introspection.write().await = Reply::json(json!({
            "active": true, "sub": subject, "permissions": ["cma:*"],
        }));
        assert_unavailable(fixture.request(state.clone()).await);
    }
    *fixture.control.userinfo.write().await = Reply::json(json!({"sub": "owner"}));
    *fixture.control.introspection.write().await = Reply::json(json!({
        "active": true, "sub": "other", "permissions": ["cma:*"],
    }));
    assert_unavailable(fixture.request(state.clone()).await);
    fixture.assert_unchanged(&state).await;

    let calls = fixture.control.requests.load(Ordering::SeqCst);
    assert_unavailable(request_for_cookie(&fixture.auth, &SubjectId::new("other"), state).await);
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst), calls);
}

#[tokio::test]
async fn provider_outages_are_sanitized_and_leave_durable_authority_unchanged() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    for endpoint in [
        &fixture.control.discovery,
        &fixture.control.userinfo,
        &fixture.control.introspection,
    ] {
        endpoint.write().await.status = StatusCode::SERVICE_UNAVAILABLE;
        assert_unavailable(fixture.request(state.clone()).await);
        endpoint.write().await.status = StatusCode::OK;
        fixture.assert_unchanged(&state).await;
    }
    let mut metadata = fixture.auth.metadata().await.unwrap();
    metadata.userinfo_endpoint = Some("http://127.0.0.1:0/userinfo".into());
    *fixture.auth.metadata.write().await = Some(metadata);
    assert_unavailable(fixture.request(state.clone()).await);
    fixture.assert_unchanged(&state).await;
}

#[tokio::test]
async fn unadvertised_introspection_uses_only_complete_userinfo_permission_evidence() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    let mut metadata = fixture.auth.metadata().await.unwrap();
    metadata.introspection_endpoint = None;
    *fixture.auth.metadata.write().await = Some(metadata);
    for body in [
        json!({"sub": "owner"}),
        json!({"sub": "owner", "permissions": ["cma:*", false]}),
    ] {
        *fixture.control.userinfo.write().await = Reply::json(body);
        assert_unavailable(fixture.request(state.clone()).await);
    }
    *fixture.control.userinfo.write().await =
        Reply::json(json!({"sub": "owner", "permissions": ["cma:profile:manage"]}));
    let request = fixture.request(state.clone()).await.unwrap();
    assert_eq!(
        request.authorization,
        Authorization::new([Capability::ManageOwnProfiles])
    );
    assert!(request.delegated_credential.is_some());
    fixture.assert_unchanged(&state).await;
}

#[tokio::test]
async fn refreshed_generation_rejects_inflight_evidence_even_when_access_text_is_reused() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    let (gate, pending) = fixture.blocked_request(state.clone()).await;
    let mut refreshed = state.clone();
    refreshed.revision += 1;
    refreshed.token_generation += 1;
    assert!(fixture
        .store
        .replace_subject(&state.incarnation, state.revision, refreshed.clone())
        .await
        .unwrap());
    gate.release.notify_one();
    assert_unavailable(pending.await.unwrap());
    fixture.assert_unchanged(&refreshed).await;
}

#[tokio::test]
async fn new_login_rejects_inflight_evidence_without_overwriting_the_login() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    let (gate, pending) = fixture.blocked_request(state.clone()).await;
    fixture
        .auth
        .store_sign_in(
            state.subject.clone(),
            state.tokens.clone().unwrap(),
            Authorization::none(),
            Profile::default(),
        )
        .await
        .unwrap();
    let winner = fixture
        .store
        .subject(&state.subject)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(winner.incarnation, state.incarnation);
    gate.release.notify_one();
    assert_unavailable(pending.await.unwrap());
    fixture.assert_unchanged(&winner).await;
}

#[tokio::test]
async fn recreated_subject_rejects_inflight_evidence_with_equal_generation_and_tokens() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    let (gate, pending) = fixture.blocked_request(state.clone()).await;
    let recreated = SubjectAuthState::new(
        state.subject.clone(),
        Authorization::none(),
        Profile::default(),
        state.tokens.clone(),
    );
    assert_eq!(recreated.token_generation, state.token_generation);
    fixture.store.put_subject(recreated.clone()).await.unwrap();
    gate.release.notify_one();
    assert_unavailable(pending.await.unwrap());
    fixture.assert_unchanged(&recreated).await;
}

#[tokio::test]
async fn durable_recheck_requires_the_same_still_usable_access_token() {
    let fixture = Fixture::new().await;
    for case in 0..3 {
        let state = fixture.seed().await;
        let (gate, pending) = fixture.blocked_request(state.clone()).await;
        let mut changed = state;
        match case {
            0 => changed.tokens = None,
            1 => changed.tokens.as_mut().unwrap().expires_at = SystemTime::UNIX_EPOCH,
            _ => changed.tokens.as_mut().unwrap().access_token = SecretValue::new("other".into()),
        }
        fixture.store.put_subject(changed.clone()).await.unwrap();
        gate.release.notify_one();
        assert_unavailable(pending.await.unwrap());
        fixture.assert_unchanged(&changed).await;
    }
}

#[tokio::test]
async fn provider_deadline_covers_discovery_userinfo_and_introspection_together() {
    let fixture = Fixture::new().await;
    let state = fixture.seed().await;
    fixture.control.discovery.write().await.delay = Duration::from_millis(80);
    fixture.control.userinfo.write().await.delay = Duration::from_millis(80);
    // Discovery remains serial, followed by concurrent provider reads. Their total
    // must still be bounded by the same per-caller deadline.
    fixture.control.introspection.write().await.delay = Duration::from_millis(160);
    assert_eq!(PROVIDER_VERIFICATION_TIMEOUT, Duration::from_secs(15));
    assert_unavailable(
        request_with_timeout(
            &fixture.auth,
            &state.subject,
            state.clone(),
            Duration::from_millis(200),
        )
        .await,
    );
    fixture.assert_unchanged(&state).await;
}
