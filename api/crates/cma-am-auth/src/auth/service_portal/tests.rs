use super::*;
use crate::auth::{
    persistence::{
        AuthStore, DurableSubjectTokens, MemoryAuthStore, SecretValue, SubjectAuthState,
    },
    profile::Profile,
    AuthMode,
};
use axum::{
    extract::Path,
    http::{HeaderValue, Request as HttpRequest},
    middleware::{from_fn_with_state, Next},
    routing::{get, post},
};
use layer2_core::{Authorization, SubjectId};
use serde_json::{json, Value};
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::SystemTime,
};
use tokio::sync::{Notify, RwLock};
use tower::ServiceExt;
#[path = "refusal_tests.rs"]
mod refusal_tests;
#[path = "source_tests.rs"]
mod source_tests;
#[path = "standing_tests.rs"]
mod standing_tests;
#[path = "streaming_tests.rs"]
mod streaming_tests;
#[derive(Default)]
struct Gate {
    reached: Notify,
    release: Notify,
}
#[derive(Default)]
struct Control {
    calls: AtomicUsize,
    refresh_calls: AtomicUsize,
    requests: RwLock<Vec<(Method, String)>>,
    headers: RwLock<HeaderMap>,
    resource: RwLock<Option<String>>,
    permissions: RwLock<Vec<String>>,
    active: RwLock<bool>,
    redirect: RwLock<bool>,
    gate: RwLock<Option<Arc<Gate>>>,
}
struct Fixture {
    auth: Arc<AuthState>,
    store: Arc<MemoryAuthStore>,
    control: Arc<Control>,
    router: Router,
    cookie: String,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    (
        origin,
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() }),
    )
}
impl Fixture {
    async fn new() -> Self {
        Self::with_standing(Arc::new(layer2_core::account_deletion::AvailableAccounts)).await
    }
    async fn with_standing(
        standing: Arc<dyn layer2_core::account_deletion::AccountStanding>,
    ) -> Self {
        let control = Arc::new(Control::default());
        *control.active.write().await = true;
        *control.permissions.write().await = vec!["cma-eg:channels:read".into()];
        let issuer = Arc::new(RwLock::new(String::new()));
        let discovery = issuer.clone();
        let introspection = issuer.clone();
        let provider = Router::new()
            .route("/.well-known/openid-configuration", get(move || { let origin=discovery.clone(); async move { let base=origin.read().await; Json(json!({"authorization_endpoint":format!("{base}/authorize"),"token_endpoint":format!("{base}/token"),"userinfo_endpoint":format!("{base}/userinfo"),"introspection_endpoint":format!("{base}/oauth/introspect")})) } }))
            .route("/userinfo", get(|| async { Json(json!({"sub":"owner"})) }))
            .route("/oauth/introspect", post(move |State(control):State<Arc<Control>>| { let issuer=introspection.clone(); async move { Json(json!({"active":*control.active.read().await,"sub":"owner","iss":*issuer.read().await,"token_type":"access","exp":SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs()+3600,"permissions":*control.permissions.read().await,"resource":control.resource.read().await.clone().map(|v|vec![v])})) } }))
            .route("/token", post(|State(control):State<Arc<Control>>| async move { control.refresh_calls.fetch_add(1, Ordering::SeqCst); Json(json!({"access_token":"renewed-test-access","refresh_token":"renewed-test-refresh","expires_in":3600})) }))
            .with_state(control.clone());
        let (base, provider_task) = serve(provider).await;
        *issuer.write().await = base.clone();
        let eg_auth = cma_service_auth::NyxIdAuth::new(
            &base,
            cma_service_auth::Service::EventGateway,
            "shared-client",
            "test-client-secret".into(),
        )
        .unwrap();
        let upstream_auth = eg_auth.clone();
        let source_auth = eg_auth.clone();
        let upstream = Router::new()
            .route(
                "/v1/channels/channel/outbound-messages/delivery/send",
                post(|State(control): State<Arc<Control>>| async move {
                    control.calls.fetch_add(1, Ordering::SeqCst);
                    Json(json!({"sent":true}))
                }),
            )
            .route(
                "/v1/channels",
                get(
                    move |State(control): State<Arc<Control>>, headers: HeaderMap| {
                        let auth = upstream_auth.clone();
                        async move {
                            control.calls.fetch_add(1, Ordering::SeqCst);
                            *control.headers.write().await = headers.clone();
                            if let Some(gate) = control.gate.read().await.clone() {
                                gate.reached.notify_one();
                                gate.release.notified().await;
                            }
                            if *control.redirect.read().await {
                                return (
                                    StatusCode::FOUND,
                                    [(header::LOCATION, "https://outside.example/")],
                                )
                                    .into_response();
                            }
                            match auth
                                .authenticate(&headers)
                                .await
                                .and_then(|p| p.require("cma-eg:channels:read"))
                            {
                                Ok(()) => (
                                    [
                                        (header::SET_COOKIE, "must-not-cross=1"),
                                        (header::CACHE_CONTROL, "public"),
                                    ],
                                    Json(json!({"items":[]})),
                                )
                                    .into_response(),
                                Err(e) => e.into_response(),
                            }
                        }
                    },
                )
                .post(|State(control): State<Arc<Control>>| async move {
                    control.calls.fetch_add(1, Ordering::SeqCst);
                    StatusCode::NO_CONTENT
                }),
            )
            .route(
                "/v1/relay-resource-connections",
                get(
                    |State(control): State<Arc<Control>>, headers: HeaderMap| async move {
                        control.calls.fetch_add(1, Ordering::SeqCst);
                        *control.headers.write().await = headers;
                        Json(json!({"items":[]}))
                    },
                ),
            )
            .route(
                "/v1/channels/{channel}/sources/{index}/test",
                post(
                    move |State(control): State<Arc<Control>>,
                          Path((channel, index)): Path<(String, usize)>,
                          headers: HeaderMap| {
                        let auth = source_auth.clone();
                        async move {
                            *control.headers.write().await = headers.clone();
                            let principal = match auth.authenticate(&headers).await {
                                Ok(principal) => principal,
                                Err(error) => return error.into_response(),
                            };
                            if let Err(error) = principal.require("cma-eg:channels:test") {
                                return error.into_response();
                            }
                            if principal.subject != "owner" || channel == "foreign" || index != 0 {
                                return (
                                    StatusCode::NOT_FOUND,
                                    Json(json!({"status":404,"detail":"Source not found."})),
                                )
                                    .into_response();
                            }
                            let platform = match channel.as_str() {
                                "saved-discord" => "discord",
                                "saved-relay" => "telegram",
                                _ => return StatusCode::NOT_FOUND.into_response(),
                            };
                            Json(source_check_response(platform)).into_response()
                        }
                    },
                ),
            )
            .merge(streaming_tests::routes(eg_auth.clone()))
            .with_state(control.clone())
            .merge(cma_service_auth::session::router(eg_auth))
            .layer(from_fn_with_state(
                control.clone(),
                |State(control): State<Arc<Control>>, request: Request, next: Next| async move {
                    control
                        .requests
                        .write()
                        .await
                        .push((request.method().clone(), request.uri().to_string()));
                    next.run(request).await
                },
            ));
        let (gateway, gateway_task) = serve(upstream).await;
        let store = Arc::new(MemoryAuthStore::default());
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: base,
                client_id: "shared-client".into(),
                client_secret: "test-client-secret".into(),
                public_url: "https://cma.example.test".into(),
            },
            None,
            store.clone(),
        )
        .with_browser_origins(&[HeaderValue::from_static("https://new-cma.example.test")])
        .with_account_standing(standing);
        store
            .put_subject(SubjectAuthState::new(
                SubjectId::new("owner"),
                Authorization::none(),
                Profile::default(),
                Some(DurableSubjectTokens {
                    access_token: SecretValue::new("initial-test-access".into()),
                    refresh_token: Some(SecretValue::new("initial-test-refresh".into())),
                    expires_at: SystemTime::now() + Duration::from_secs(3600),
                }),
            ))
            .await
            .unwrap();
        let cookie = auth
            .issue_browser_login(SubjectId::new("owner"))
            .await
            .unwrap();
        let router = router(auth.clone(), Some(&gateway)).unwrap();
        Self {
            auth,
            store,
            control,
            router,
            cookie,
            tasks: vec![provider_task, gateway_task],
        }
    }
    fn request(&self, method: Method, path: &str) -> HttpRequest<Body> {
        HttpRequest::builder()
            .method(method)
            .uri(format!("{PREFIX}{path}"))
            .header(header::COOKIE, format!("cma_session={}", self.cookie))
            .body(Body::empty())
            .unwrap()
    }
    async fn send(&self, r: HttpRequest<Body>) -> Response {
        self.router.clone().oneshot(r).await.unwrap()
    }
    fn post(&self, path: &str) -> HttpRequest<Body> {
        let mut request = self.request(Method::POST, path);
        request.headers_mut().insert(
            header::ORIGIN,
            HeaderValue::from_static("https://cma.example.test"),
        );
        request
    }
}

fn source_check_response(platform: &str) -> Value {
    json!({
        "source_index": 0,
        "platform": platform,
        "scope": "configuration_only",
        "external_delivery_tested": false,
        "checks": [{"id":"credential", "label":"Credential", "status":"unavailable", "detail":"A credential is not configured."}]
    })
}

#[tokio::test]
async fn existing_login_reaches_gateway_with_no_credentials_in_browser_response() {
    let f = Fixture::new().await;
    let mut r = f.request(Method::GET, "/v1/channels");
    r.headers_mut()
        .insert("x-cmaeg-owner", HeaderValue::from_static("other-owner"));
    r.headers_mut()
        .insert("x-forwarded-user", HeaderValue::from_static("other-owner"));
    let response = f.send(r).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    {
        let seen = f.control.headers.read().await;
        assert_eq!(seen[header::AUTHORIZATION], "Bearer initial-test-access");
        assert!(!seen.contains_key(header::COOKIE));
        assert!(!seen.contains_key("x-cmaeg-owner"));
        assert!(!seen.contains_key("x-forwarded-user"));
    }
    let body = to_bytes(response.into_body(), BODY_LIMIT).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"items":[]})
    );
    let session = f.send(f.request(Method::GET, "/auth/session")).await;
    assert_eq!(session.status(), StatusCode::OK);
    let body = to_bytes(session.into_body(), BODY_LIMIT).await.unwrap();
    assert!(!String::from_utf8_lossy(&body).contains("test-access"));
}
#[tokio::test]
async fn independent_audience_and_gateway_permissions_remain_required() {
    let f = Fixture::new().await;
    *f.control.permissions.write().await = vec!["cma:admin".into()];
    assert_eq!(
        f.send(f.request(Method::GET, "/v1/channels"))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    *f.control.permissions.write().await = vec!["cma-eg:channels:read".into()];
    *f.control.resource.write().await = Some("https://wrong.example/resource".into());
    assert_eq!(
        f.send(f.request(Method::GET, "/v1/channels"))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}
#[tokio::test]
async fn writes_require_exact_origin_and_disallowed_paths_never_reach_gateway() {
    let f = Fixture::new().await;
    for origin in [None, Some("https://other.example"), Some("null")] {
        let mut r = f.request(Method::POST, "/v1/channels");
        if let Some(o) = origin {
            r.headers_mut().insert(header::ORIGIN, o.parse().unwrap());
        }
        assert_eq!(f.send(r).await.status(), StatusCode::FORBIDDEN);
    }
    for path in [
        "/auth/login",
        "/auth/logout",
        "/mcp",
        "/v1/channels/a/rekey",
        "/v1/channels/../secret",
        "/v1/channels/a%2Fb",
    ] {
        assert_eq!(
            f.send(f.request(Method::GET, path)).await.status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(f.control.calls.load(Ordering::SeqCst), 0);
    for origin in ["https://cma.example.test", "https://new-cma.example.test"] {
        let mut r = f.request(Method::POST, "/v1/channels");
        r.headers_mut()
            .insert(header::ORIGIN, origin.parse().unwrap());
        assert_eq!(f.send(r).await.status(), StatusCode::NO_CONTENT);
    }
}
#[tokio::test]
async fn duplicate_cookie_or_explicit_bearer_never_selects_another_identity() {
    let f = Fixture::new().await;
    let mut duplicate = f.request(Method::GET, "/v1/channels");
    duplicate.headers_mut().append(
        header::COOKIE,
        HeaderValue::from_static("cma_session=another"),
    );
    assert_eq!(f.send(duplicate).await.status(), StatusCode::UNAUTHORIZED);
    let mut bearer = f.request(Method::GET, "/v1/channels");
    bearer.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer another"),
    );
    assert_eq!(f.send(bearer).await.status(), StatusCode::UNAUTHORIZED);
    let mut old = f.request(Method::GET, "/v1/channels");
    old.headers_mut().insert(
        header::COOKIE,
        HeaderValue::from_static("__Host-cma-event-gateway=old-independent-cookie"),
    );
    assert_eq!(f.send(old).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(f.control.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn logout_and_inactive_provider_evidence_revoke_gateway_access() {
    let f = Fixture::new().await;
    *f.control.active.write().await = false;
    assert_eq!(
        f.send(f.request(Method::GET, "/v1/channels"))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    *f.control.active.write().await = true;
    f.auth.revoke_browser_login(&f.cookie).await.unwrap();
    assert_eq!(
        f.send(f.request(Method::GET, "/v1/channels"))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(f.control.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn existing_cma_refresh_rotates_token_before_forwarding() {
    let f = Fixture::new().await;
    let mut state = f
        .store
        .subject(&SubjectId::new("owner"))
        .await
        .unwrap()
        .unwrap();
    state.tokens.as_mut().unwrap().expires_at = SystemTime::now() + Duration::from_secs(10);
    f.store.put_subject(state).await.unwrap();
    assert_eq!(
        f.send(f.request(Method::GET, "/v1/channels"))
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        f.control.headers.read().await[header::AUTHORIZATION],
        "Bearer renewed-test-access"
    );
}

#[tokio::test]
async fn resource_discovery_uses_existing_refreshed_custody_without_forwarding_owner_or_secrets() {
    let f = Fixture::new().await;
    let mut state = f
        .store
        .subject(&SubjectId::new("owner"))
        .await
        .unwrap()
        .unwrap();
    state.tokens.as_mut().unwrap().expires_at = SystemTime::now() + Duration::from_secs(10);
    f.store.put_subject(state).await.unwrap();
    let mut request = f.request(
        Method::GET,
        "/v1/relay-resource-connections?platform=feishu",
    );
    request
        .headers_mut()
        .insert("x-cmaeg-owner", HeaderValue::from_static("another-owner"));
    let response = f.send(request).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let body = to_bytes(response.into_body(), 1024).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"items":[]})
    );
    let headers = f.control.headers.read().await;
    assert_eq!(headers[header::AUTHORIZATION], "Bearer renewed-test-access");
    assert!(!headers.contains_key("x-cmaeg-owner"));
    assert!(!headers.contains_key(header::COOKIE));
    assert_eq!(f.control.calls.load(Ordering::SeqCst), 1);
    assert!(!allowed(&Method::POST, "/v1/relay-resource-connections"));
}
#[tokio::test]
async fn upstream_redirects_cannot_exfiltrate_server_bearer() {
    let f = Fixture::new().await;
    *f.control.redirect.write().await = true;
    let response = f.send(f.request(Method::GET, "/v1/channels")).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert!(!response.headers().contains_key(header::LOCATION));
}

#[tokio::test]
async fn logout_during_upstream_read_cannot_restore_private_data() {
    let f = Fixture::new().await;
    let gate = Arc::new(Gate::default());
    *f.control.gate.write().await = Some(gate.clone());
    let router = f.router.clone();
    let request = f.request(Method::GET, "/v1/channels");
    let pending = tokio::spawn(async move { router.oneshot(request).await.unwrap() });
    tokio::time::timeout(Duration::from_secs(2), gate.reached.notified())
        .await
        .unwrap();
    f.auth.revoke_browser_login(&f.cookie).await.unwrap();
    gate.release.notify_one();
    let response = pending.await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        !String::from_utf8_lossy(&to_bytes(response.into_body(), BODY_LIMIT).await.unwrap())
            .contains("items")
    );
}

#[tokio::test]
async fn oversized_bodies_are_rejected_before_dispatch() {
    let f = Fixture::new().await;
    let mut request = f.request(Method::POST, "/v1/channels");
    request.headers_mut().insert(
        header::ORIGIN,
        HeaderValue::from_static("https://cma.example.test"),
    );
    *request.body_mut() = Body::from(vec![b'x'; BODY_LIMIT + 1]);
    assert_eq!(
        f.send(request).await.status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(f.control.calls.load(Ordering::SeqCst), 0);
}
