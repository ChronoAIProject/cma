use super::*;
use crate::auth::persistence::{
    AuthStore, DurableSubjectTokens, MemoryAuthStore, SubjectAuthState,
};
use axum::{
    extract::{Form, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use tokio::sync::Mutex;

mod relogin;

const DRIVE: &str = "00000000-0000-4000-8000-000000000001";
const GITHUB: &str = "00000000-0000-4000-8000-000000000002";
struct Peer {
    discovery: Mutex<Value>,
    consents: Mutex<Value>,
    services: Mutex<Value>,
    status: Mutex<StatusCode>,
    exchange: Mutex<Vec<BTreeMap<String, String>>>,
    identity: Mutex<Value>,
    retained_identity: Mutex<Value>,
}
struct Fixture {
    auth: Arc<AuthState>,
    store: Arc<MemoryAuthStore>,
    peer: Arc<Peer>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
fn grant(ids: &[&str], expiry: Option<&str>) -> Value {
    json!({"client_id":"cma-fixture","scopes":"openid profile roles proxy",
        "allow_all_services":false,"allowed_service_ids":ids,"legacy_unrestricted":false,"expires_at":expiry})
}
fn service(id: &str, slug: &str) -> Value {
    json!({"id":id,"slug":slug,"resource_uri":format!("https://api.identity.example/api/v1/proxy/s/{slug}"),
        "is_active":true,"credential_source":{"type":"personal"}})
}
fn identity() -> Value {
    json!({"active":true,"sub":"owner","client_id":null,"iss":"https://identity.example",
        "token_type":"access","exp":4102444800_u64,"permissions":["cma:*"]})
}
async fn consents(State(peer): State<Arc<Peer>>) -> impl IntoResponse {
    (
        *peer.status.lock().await,
        Json(peer.consents.lock().await.clone()),
    )
}
async fn introspect(
    State(peer): State<Arc<Peer>>,
    Form(form): Form<BTreeMap<String, String>>,
) -> Json<Value> {
    assert_eq!(form["client_id"], "cma-fixture");
    Json(if form["token"] == "exchanged-fixture" {
        peer.identity.lock().await.clone()
    } else if form["token"] == "retained-fixture" || form["token"] == "refreshed-fixture" {
        peer.retained_identity.lock().await.clone()
    } else {
        identity()
    })
}
impl Fixture {
    async fn new() -> Self {
        let peer = Arc::new(Peer {
            discovery: Mutex::new(json!({
                "issuer":"https://identity.example", "authorization_endpoint":"https://api.identity.example/oauth/authorize",
                "token_endpoint":"https://api.identity.example/oauth/token", "introspection_endpoint":"https://api.identity.example/oauth/introspect",
                "userinfo_endpoint":"https://api.identity.example/oauth/userinfo"})),
            consents: Mutex::new(json!({"consents":[grant(&[DRIVE],None)]})),
            services: Mutex::new(
                json!({"services":[service(DRIVE,"api-drive"),service(GITHUB,"api-github")]}),
            ),
            status: Mutex::new(StatusCode::OK),
            exchange: Mutex::new(Vec::new()),
            identity: Mutex::new(identity()),
            retained_identity: Mutex::new(identity()),
        });
        let app =
            Router::new()
                .route(
                    "/.well-known/openid-configuration",
                    get(|State(peer): State<Arc<Peer>>| async move {
                        Json(peer.discovery.lock().await.clone())
                    }),
                )
                .route(
                    "/oauth/userinfo",
                    get(|| async { Json(json!({"sub":"owner"})) }),
                )
                .route("/oauth/introspect", post(introspect))
                .route("/api/v1/users/me/consents", get(consents))
                .route(
                    "/api/v1/user-services",
                    get(|State(peer): State<Arc<Peer>>| async move {
                        Json(peer.services.lock().await.clone())
                    }),
                )
                .route(
                    "/api/v1/proxy/s/api-github",
                    get(|headers: axum::http::HeaderMap| async move {
                        if headers.get("authorization").is_some_and(|value| value == "Bearer new-sign-in-fixture") {
                            StatusCode::NO_CONTENT
                        } else {
                            StatusCode::FORBIDDEN
                        }
                    }),
                )
                .route(
                    "/oauth/token",
                    post(
                        |State(peer): State<Arc<Peer>>,
                         Form(form): Form<BTreeMap<String, String>>| async move {
                            let token = if form.get("grant_type").is_some_and(|value| value == "refresh_token") {
                                "refreshed-fixture"
                            } else if form.get("redirect_uri").is_some_and(|value| value.ends_with("/auth/callback")) {
                                "new-sign-in-fixture"
                            } else {
                                "exchanged-fixture"
                            };
                            peer.exchange.lock().await.push(form);
                            Json(json!({"access_token":token,"refresh_token":"refreshed-refresh-fixture","expires_in":3600}))
                        },
                    ),
                )
                .with_state(peer.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let store = Arc::new(MemoryAuthStore::default());
        let auth = AuthState::with_store_and_nyxid_public_url(
            AuthMode::Provider {
                base_url: base,
                client_id: "cma-fixture".into(),
                client_secret: random_token(),
                public_url: "https://app.example".into(),
            },
            None,
            store.clone(),
            Some("https://login.identity.example".into()),
        )
        .with_browser_origins(&["https://app.example".parse().unwrap()])
        .with_service_authorization(
            true,
            Some(&cma_am_auth::service_config::ServiceAuthorizationConfig {
                issuer: Url::parse("https://identity.example/").unwrap(),
                api_origin: Url::parse("https://api.identity.example/").unwrap(),
            }),
        );
        store
            .put_subject(SubjectAuthState::new(
                SubjectId::new("owner"),
                AuthState::trusted_authorization(),
                Default::default(),
                Some(DurableSubjectTokens {
                    access_token: SecretValue::new("retained-fixture".into()),
                    refresh_token: Some(SecretValue::new("retained-refresh-fixture".into())),
                    expires_at: SystemTime::now() + Duration::from_secs(3600),
                }),
            ))
            .await
            .unwrap();
        Self {
            auth,
            store,
            peer,
            server,
        }
    }
    fn intent(&self) -> RecoveryIntent {
        RecoveryIntent {
            subject: SubjectId::new("owner"),
            issuer: "https://identity.example/".into(),
            required: [ServiceId::new("api-drive"), ServiceId::new("api-github")].into(),
            requested: [ServiceId::new("api-github")].into(),
            redirect_uri: "https://app.example/auth/service-authorization/callback".into(),
            return_to: "/".into(),
        }
    }
    fn authority(&self) -> &dyn GrantAuthority {
        self.auth.fixture_grant_authority().unwrap()
    }
}

#[tokio::test]
async fn live_grant_requests_only_exact_missing_uuid_and_preserves_original_login() {
    let f = Fixture::new().await;
    let intent = f.intent();
    let original = f.store.subject(&intent.subject).await.unwrap().unwrap();
    assert_eq!(
        f.authority()
            .account(&f.auth, &intent.subject)
            .await
            .unwrap()
            .unwrap()
            .management_url
            .unwrap()
            .as_str(),
        "https://login.identity.example/settings/consents"
    );
    assert_eq!(
        f.authority()
            .missing(&f.auth, &intent)
            .await
            .unwrap()
            .unwrap(),
        intent.requested
    );
    let url = f
        .authority()
        .consent_url(&f.auth, &intent, "state", "challenge")
        .await
        .unwrap()
        .unwrap();
    let query: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(query["requested_service_ids"], GITHUB);
    assert_eq!(query["service_access_mode"], "incremental");
    assert_eq!(query["scope"], "openid profile roles proxy");
    assert!(!query.contains_key("resource"));
    let identity = f
        .authority()
        .redeem(&f.auth, &intent, "code", &SecretValue::new("proof".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(identity.subject, intent.subject);
    let posted = f.peer.exchange.lock().await;
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0]["redirect_uri"], intent.redirect_uri);
    assert_eq!(posted[0]["code_verifier"], "proof");
    let retained = f.store.subject(&intent.subject).await.unwrap().unwrap();
    assert_eq!(retained.tokens, original.tokens);
    assert_eq!(retained.incarnation, original.incarnation);
}

#[tokio::test]
async fn absent_and_expired_grants_use_resource_consent_and_never_report_revocation() {
    let f = Fixture::new().await;
    for (consents, status) in [
        (json!([]), "not_granted"),
        (
            json!([grant(&[DRIVE], Some("2020-01-01T00:00:00Z"))]),
            "expired",
        ),
    ] {
        *f.peer.consents.lock().await = json!({"consents":consents});
        let grant = f
            .authority()
            .account(&f.auth, &SubjectId::new("owner"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(serde_json::to_value(grant.status).unwrap(), status);
        let mut intent = f.intent();
        intent.requested = intent.required.clone();
        let url = f
            .authority()
            .consent_url(&f.auth, &intent, "state", "challenge")
            .await
            .unwrap()
            .unwrap();
        let query: Vec<_> = url.query_pairs().into_owned().collect();
        assert!(!query
            .iter()
            .any(|(key, _)| key == "service_access_mode" || key == "requested_service_ids"));
        assert_eq!(query.iter().filter(|(key, _)| key == "resource").count(), 2);
        assert!(query
            .iter()
            .any(|(key, value)| key == "prompt" && value == "consent"));
    }
}

#[tokio::test]
async fn provider_failures_and_ambiguous_or_denied_services_never_attest_missing_grants() {
    let f = Fixture::new().await;
    for status in [
        StatusCode::NOT_FOUND,
        StatusCode::UNAUTHORIZED,
        StatusCode::FORBIDDEN,
        StatusCode::SERVICE_UNAVAILABLE,
    ] {
        *f.peer.status.lock().await = status;
        assert!(f
            .authority()
            .account(&f.auth, &SubjectId::new("owner"))
            .await
            .is_err());
        assert!(f.authority().missing(&f.auth, &f.intent()).await.is_err());
    }
    *f.peer.status.lock().await = StatusCode::OK;
    let mut denied = service(GITHUB, "api-github");
    denied["credential_source"] = json!({"type":"org","allowed":false});
    for rows in [
        json!([service(DRIVE, "api-drive"), denied]),
        json!([
            service(DRIVE, "api-drive"),
            service(GITHUB, "api-github"),
            service(DRIVE, "api-github")
        ]),
    ] {
        *f.peer.services.lock().await = json!({"services":rows});
        assert!(f.authority().missing(&f.auth, &f.intent()).await.is_err());
    }
}

#[tokio::test]
async fn changed_subject_client_issuer_and_redirect_cannot_complete_consent() {
    let f = Fixture::new().await;
    for (field, value) in [
        ("sub", json!("other-owner")),
        ("client_id", json!("other-client")),
        ("iss", json!("https://other.example")),
        ("active", json!(false)),
        ("exp", json!(1)),
        ("token_type", json!("refresh")),
    ] {
        let mut proof = identity();
        proof[field] = value;
        *f.peer.identity.lock().await = proof;
        assert!(f
            .authority()
            .redeem(
                &f.auth,
                &f.intent(),
                "code",
                &SecretValue::new("proof".into())
            )
            .await
            .unwrap()
            .is_none());
    }
    let count = f.peer.exchange.lock().await.len();
    for redirect in [
        "https://other.example/auth/service-authorization/callback",
        "https://app.example/auth/callback",
    ] {
        let mut intent = f.intent();
        intent.redirect_uri = redirect.into();
        assert!(f
            .authority()
            .redeem(&f.auth, &intent, "code", &SecretValue::new("proof".into()))
            .await
            .unwrap()
            .is_none());
        assert!(f
            .authority()
            .consent_url(&f.auth, &intent, "state", "challenge")
            .await
            .unwrap()
            .is_none());
    }
    assert_eq!(f.peer.exchange.lock().await.len(), count);
}

#[tokio::test]
async fn changed_or_legacy_grant_does_not_emit_a_stale_incremental_request() {
    let f = Fixture::new().await;
    *f.peer.consents.lock().await = json!({"consents":[grant(&[DRIVE,GITHUB],None)]});
    assert!(f
        .authority()
        .consent_url(&f.auth, &f.intent(), "state", "challenge")
        .await
        .unwrap()
        .is_none());
    let mut legacy = grant(&[], None);
    legacy["legacy_unrestricted"] = true.into();
    *f.peer.consents.lock().await = json!({"consents":[legacy]});
    assert!(f.authority().missing(&f.auth, &f.intent()).await.is_err());
    let mut login_only = grant(&[DRIVE, GITHUB], None);
    login_only["scopes"] = "openid profile".into();
    *f.peer.consents.lock().await = json!({"consents":[login_only]});
    assert!(f.authority().missing(&f.auth, &f.intent()).await.is_err());
}

#[tokio::test]
async fn unsupported_management_credentials_do_not_mean_revoked_or_permit_recovery() {
    let f = Fixture::new().await;
    *f.peer.status.lock().await = StatusCode::FORBIDDEN;
    *f.peer.consents.lock().await = json!({"error":"forbidden","details":{
        "reason":"credential_type_unsupported","credential_type":"oauth_client",
        "accepted":["user_session"]}});
    let account = f
        .authority()
        .account(&f.auth, &SubjectId::new("owner"))
        .await
        .unwrap()
        .unwrap();
    assert!(account.management_credential_unsupported);
    assert_eq!(serde_json::to_value(account.status).unwrap(), "unknown");
    assert!(f.authority().missing(&f.auth, &f.intent()).await.is_err());
    assert!(f
        .authority()
        .consent_url(&f.auth, &f.intent(), "state", "challenge")
        .await
        .is_err());
    assert!(f.peer.exchange.lock().await.is_empty());
}

#[tokio::test]
async fn discovery_cannot_redirect_consent_or_exchange_outside_the_pinned_origins() {
    let f = Fixture::new().await;
    let original = f.peer.discovery.lock().await.clone();
    for field in [
        "issuer",
        "authorization_endpoint",
        "token_endpoint",
        "introspection_endpoint",
    ] {
        let mut changed = original.clone();
        changed[field] = "https://unexpected.example/oauth".into();
        *f.peer.discovery.lock().await = changed;
        assert!(f
            .authority()
            .consent_url(&f.auth, &f.intent(), "state", "challenge")
            .await
            .is_err());
        assert!(f
            .authority()
            .redeem(
                &f.auth,
                &f.intent(),
                "code",
                &SecretValue::new("proof".into())
            )
            .await
            .is_err());
    }
    assert!(f.peer.exchange.lock().await.is_empty());
}
