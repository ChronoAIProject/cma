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
#[path = "cookie_authorization_tests/connection_route_tests.rs"]
mod connection_route_tests;
#[path = "cookie_authorization_tests/deferred_command_tests.rs"]
mod deferred_command_tests;
#[path = "cookie_authorization_tests/gateway_session_tests.rs"]
mod gateway_session_tests;
#[path = "cookie_authorization_tests/personal_owner_tests.rs"]
mod personal_owner_tests;
#[path = "cookie_authorization_tests/protected_outage_counts.rs"]
mod protected_outage_counts;
#[path = "cookie_authorization_tests/protected_outage_tests.rs"]
mod protected_outage_tests;
#[path = "cookie_authorization_tests/provider_overlap_tests.rs"]
mod provider_overlap_tests;
#[path = "cookie_authorization_tests/resource_command_tests.rs"]
mod resource_command_tests;
#[path = "cookie_authorization_tests/session_routes_tests.rs"]
mod session_routes_tests;
#[path = "cookie_authorization_tests/tenant_isolation_tests.rs"]
mod tenant_isolation_tests;

#[path = "cookie_authorization_tests/cause_diagnostic_tests.rs"]
mod cause_diagnostic_tests;
#[path = "cookie_authorization_tests/diagnostic_tests.rs"]
mod diagnostic_tests;
#[path = "cookie_authorization_tests/final_diagnostic_tests.rs"]
mod final_diagnostic_tests;
#[path = "cookie_authorization_tests/provider_diagnostic_tests.rs"]
mod provider_diagnostic_tests;

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
    headers: HeaderMap,
    stream_tail: Option<(Duration, Result<String, &'static str>)>,
}

impl Reply {
    fn json(body: Value) -> Self {
        Self {
            status: StatusCode::OK,
            body: body.to_string(),
            delay: Duration::ZERO,
            gate: None,
            headers: HeaderMap::new(),
            stream_tail: None,
        }
    }
}

struct ProviderControl {
    scope_allowed: std::sync::atomic::AtomicBool,
    discovery: RwLock<Reply>,
    userinfo: RwLock<Reply>,
    introspection: RwLock<Reply>,
    introspection_arrival: RwLock<Option<Arc<Gate>>>,
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
    use axum::body::Body;
    use futures_util::StreamExt;
    let body = match reply.stream_tail {
        Some((delay, tail)) => Body::from_stream(
            futures_util::stream::once(async move { Ok::<_, std::io::Error>(reply.body) }).chain(
                futures_util::stream::once(async move {
                    tokio::time::sleep(delay).await;
                    tail.map_err(std::io::Error::other)
                }),
            ),
        ),
        None => Body::from(reply.body),
    };
    let mut response = (
        reply.status,
        [(header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response();
    response.headers_mut().extend(reply.headers);
    response
}

async fn discovery(State(control): State<Arc<ProviderControl>>) -> Response {
    respond(&control, &control.discovery).await
}

async fn userinfo(State(control): State<Arc<ProviderControl>>) -> Response {
    respond(&control, &control.userinfo).await
}

async fn introspection(State(control): State<Arc<ProviderControl>>) -> Response {
    let gate = control.introspection_arrival.read().await.clone();
    if let Some(gate) = gate {
        gate.reached.notify_one();
        gate.release.notified().await;
    }
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
            scope_allowed: std::sync::atomic::AtomicBool::new(true),
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
            introspection_arrival: RwLock::new(None),
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
            .route("/oauth/introspect", post(introspection))
            .route("/token", post(exchange))
            .route(
                "/api/v1/user-services",
                get(|| async {
                    axum::Json(json!([{"id":"connected-github","slug":"github","is_active":true}]))
                }),
            )
            .route(
                "/api/v1/api-keys/scope-plan",
                post(
                    |State(control): State<Arc<ProviderControl>>,
                     axum::Json(body): axum::Json<Value>| async move {
                        if !control.scope_allowed.load(Ordering::SeqCst) {
                            return StatusCode::FORBIDDEN.into_response();
                        }
                        axum::Json(crate::auth::keys::scope_plan_fixture(
                            "owner",
                            &body,
                            &["owner-node"],
                        ))
                        .into_response()
                    },
                ),
            )
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
            assert!(matches!(
                message.as_str(),
                "cookie authorization could not be verified" | "identity_provider_unavailable"
            ));
        }
        Err(error) => panic!("expected sanitized unavailable, got {error:?}"),
        Ok(_) => panic!("unverifiable provider evidence must not authenticate the cookie"),
    }
}

#[path = "cookie_authorization_tests/fallback_tests.rs"]
mod fallback_tests;
#[path = "cookie_authorization_tests/verification_tests.rs"]
mod verification_tests;

#[path = "cookie_authorization_tests/provider_boundary_tests.rs"]
mod provider_boundary_tests;

#[path = "cookie_authorization_tests/joined_session_tests.rs"]
mod joined_session_tests;

#[path = "cookie_authorization_tests/throttle_tests.rs"]
mod throttle_tests;

#[path = "cookie_authorization_tests/cooldown_tests.rs"]
mod cooldown_tests;

#[path = "cookie_authorization_tests/live_evidence_tests.rs"]
mod live_evidence_tests;
#[path = "cookie_authorization_tests/replica_verification_tests.rs"]
mod replica_verification_tests;
