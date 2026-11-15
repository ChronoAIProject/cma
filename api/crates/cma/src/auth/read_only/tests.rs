use super::*;
use crate::auth::{
    persistence::{AuthStore, BrowserLogin, DurableSubjectTokens, MemoryAuthStore, SecretValue},
    profile::Profile,
};
use axum::{
    body::Body,
    http::Request as HttpRequest,
    routing::{get, post},
    Json, Router,
};
use layer2_core::{Authorization, Capability, SubjectId};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

struct Fixture {
    auth: Arc<AuthState>,
    store: Arc<MemoryAuthStore>,
    headers: HeaderMap,
    login: SecretDigest,
    calls: Arc<AtomicUsize>,
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
        let discovery = base.clone();
        let issuer = base.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let user_calls = calls.clone();
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(move || { let base = discovery.clone(); async move {
                Json(json!({"authorization_endpoint": format!("{base}/authorize"), "token_endpoint": format!("{base}/token"), "userinfo_endpoint": format!("{base}/userinfo"), "introspection_endpoint": format!("{base}/introspect")}))
            }}))
            .route("/userinfo", get(move || { let calls = user_calls.clone(); async move { calls.fetch_add(1, Ordering::SeqCst); Json(json!({"sub": "owner"})) } }))
            .route("/introspect", post(move || { let issuer = issuer.clone(); async move { Json(json!({"active":true,"sub":"owner","iss":issuer,"token_type":"access","exp":SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs()+3600,"permissions":["cma:*"]})) } }));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let store = Arc::new(MemoryAuthStore::default());
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: base,
                client_id: "test-client".into(),
                client_secret: uuid::Uuid::new_v4().to_string(),
                public_url: "https://cma.example.test".into(),
            },
            None,
            store.clone(),
        );
        store
            .put_subject(SubjectAuthState::new(
                SubjectId::new("owner"),
                Authorization::none(),
                Profile::default(),
                Some(DurableSubjectTokens {
                    access_token: SecretValue::new(uuid::Uuid::new_v4().to_string()),
                    refresh_token: Some(SecretValue::new(uuid::Uuid::new_v4().to_string())),
                    expires_at: SystemTime::now() + Duration::from_secs(3600),
                }),
            ))
            .await
            .unwrap();
        let cookie = uuid::Uuid::new_v4().to_string();
        let login = SecretDigest::identify(crate::auth::AUTH_COOKIE_DOMAIN, &cookie);
        store
            .put_browser_login(BrowserLogin {
                id: login.clone(),
                subject: SubjectId::new("owner"),
                expires_at: SystemTime::now() + Duration::from_secs(3600),
            })
            .await
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("{}={cookie}", crate::auth::AUTH_COOKIE)
                .parse()
                .unwrap(),
        );
        Self {
            auth,
            store,
            headers,
            login,
            calls,
            server,
        }
    }

    fn request(&self) -> HttpRequest<Body> {
        let mut request = HttpRequest::builder()
            .uri("/read")
            .body(Body::empty())
            .unwrap();
        *request.headers_mut() = self.headers.clone();
        request
    }
}

#[tokio::test]
async fn fresh_existing_cookie_reads_without_changing_custody() {
    let f = Fixture::new().await;
    let before = f
        .store
        .subject(&SubjectId::new("owner"))
        .await
        .unwrap()
        .unwrap();
    let proof = authenticate(f.auth.clone(), &f.headers).await.unwrap();
    assert_eq!(proof.caller().subject, before.subject);
    assert!(proof
        .recheck()
        .await
        .unwrap()
        .authorization
        .allows(Capability::ConfigurePlatform));
    let after = f.store.subject(&before.subject).await.unwrap().unwrap();
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.token_generation, before.token_generation);
    assert_eq!(after.tokens, before.tokens);
    assert!(f.calls.load(Ordering::SeqCst) >= 2);
}

#[tokio::test]
async fn expired_or_refresh_due_cookie_refuses_without_provider_or_refresh() {
    let f = Fixture::new().await;
    for remaining in [0, 30] {
        let mut state = f
            .store
            .subject(&SubjectId::new("owner"))
            .await
            .unwrap()
            .unwrap();
        state.tokens.as_mut().unwrap().expires_at =
            SystemTime::now() + Duration::from_secs(remaining);
        f.store.put_subject(state).await.unwrap();
        assert!(matches!(
            authenticate(f.auth.clone(), &f.headers).await,
            Err(StatusCode::UNAUTHORIZED)
        ));
    }
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn logout_during_read_suppresses_the_completed_private_response() {
    let f = Fixture::new().await;
    let store = f.store.clone();
    let login = f.login.clone();
    let app = Router::new()
        .route(
            "/read",
            get(move || {
                let store = store.clone();
                let login = login.clone();
                async move {
                    store.delete_browser_login(&login).await.unwrap();
                    "private-result"
                }
            }),
        )
        .layer(axum::middleware::from_fn_with_state(f.auth.clone(), attach));
    let response = app.oneshot(f.request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert!(axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn generation_replacement_after_read_refuses_both_http_and_operator_proof() {
    let f = Fixture::new().await;
    let proof = authenticate(f.auth.clone(), &f.headers).await.unwrap();
    let store = f.store.clone();
    let app = Router::new()
        .route(
            "/read",
            get(move || {
                let store = store.clone();
                async move {
                    let mut state = store
                        .subject(&SubjectId::new("owner"))
                        .await
                        .unwrap()
                        .unwrap();
                    state.revision += 1;
                    state.token_generation += 1;
                    store.put_subject(state).await.unwrap();
                    "private-result"
                }
            }),
        )
        .layer(axum::middleware::from_fn_with_state(f.auth.clone(), attach));
    assert_eq!(
        app.oneshot(f.request()).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(matches!(
        proof.recheck().await,
        Err(StatusCode::UNAUTHORIZED)
    ));
}

#[test]
fn ambiguous_or_foreign_credentials_never_select_an_ambient_identity() {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        "cma_session=one; cma_session=two".parse().unwrap(),
    );
    assert!(cookie(&headers).is_err());
    headers.insert(header::COOKIE, "cma_session=one".parse().unwrap());
    assert!(cookie(&headers).is_ok());
    headers.insert(header::AUTHORIZATION, "Bearer fixture".parse().unwrap());
    assert!(cookie(&headers).is_err());
}
