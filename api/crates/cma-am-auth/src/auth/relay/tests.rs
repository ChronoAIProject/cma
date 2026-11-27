use super::*;
use crate::auth::{profile::Profile, ProviderMetadata};
use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use layer2_core::{Authorization, SubjectId};
use serde_json::json;
use std::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};
use tokio::sync::Notify;

#[derive(Default)]
struct Provider {
    block: AtomicBool,
    denied: AtomicBool,
    userinfo_calls: AtomicUsize,
    introspection_calls: AtomicUsize,
    entered: Notify,
    release: Notify,
}
async fn userinfo(State(state): State<Arc<Provider>>) -> Json<serde_json::Value> {
    state.userinfo_calls.fetch_add(1, Ordering::SeqCst);
    if state.block.load(Ordering::SeqCst) {
        state.entered.notify_one();
        state.release.notified().await;
    }
    Json(json!({"sub":"owner"}))
}
async fn introspect(State(state): State<Arc<Provider>>) -> Json<serde_json::Value> {
    state.introspection_calls.fetch_add(1, Ordering::SeqCst);
    Json(
        json!({"sub":"owner", "active":!state.denied.load(Ordering::SeqCst), "permissions":["cma:sandbox:use"]}),
    )
}
pub struct Fixture {
    pub auth: Arc<AuthState>,
    pub store: Arc<MemoryAuthStore>,
    provider: Arc<Provider>,
    pub headers: HeaderMap,
    pub context: CallerContext,
    login: SecretDigest,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    pub fn provider_calls(&self) -> (usize, usize) {
        (
            self.provider.userinfo_calls.load(Ordering::SeqCst),
            self.provider.introspection_calls.load(Ordering::SeqCst),
        )
    }

    pub async fn logout(&self) {
        self.store.delete_browser_login(&self.login).await.unwrap();
    }

    pub async fn new(browser: bool) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let provider = Arc::new(Provider::default());
        let router = Router::new()
            .route("/userinfo", get(userinfo))
            .route("/introspect", post(introspect))
            .with_state(provider.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let store = Arc::new(MemoryAuthStore::default());
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: origin.clone(),
                client_id: "test".into(),
                client_secret: "test".into(),
                public_url: "http://localhost".into(),
            },
            None,
            store.clone(),
        );
        *auth.metadata.write().await = Some(ProviderMetadata {
            authorization_endpoint: format!("{origin}/authorize"),
            token_endpoint: format!("{origin}/token"),
            userinfo_endpoint: Some(format!("{origin}/userinfo")),
            introspection_endpoint: Some(format!("{origin}/introspect")),
        });
        let login = SecretDigest::identify(super::super::AUTH_COOKIE_DOMAIN, "test-login");
        store
            .put_browser_login(BrowserLogin {
                id: login.clone(),
                subject: SubjectId::new("owner"),
                expires_at: SystemTime::now() + Duration::from_secs(3600),
            })
            .await
            .unwrap();
        store
            .put_subject(SubjectAuthState::new(
                SubjectId::new("owner"),
                Authorization::none(),
                Profile::default(),
                Some(DurableSubjectTokens {
                    access_token: SecretValue::new("test-access".into()),
                    refresh_token: None,
                    expires_at: SystemTime::now() + Duration::from_secs(3600),
                }),
            ))
            .await
            .unwrap();
        let mut headers = HeaderMap::new();
        if browser {
            headers.insert("cookie", "cma_session=test-login".parse().unwrap());
        } else {
            headers.insert("authorization", "Bearer test-access".parse().unwrap());
        }
        let resolved = auth.resolve_request(&headers).await.unwrap().unwrap();
        assert_eq!(
            resolved.delegated_credential.as_deref(),
            Some("test-access")
        );
        let context = auth.resource_request_context(&resolved);
        Self {
            auth,
            store,
            provider,
            headers,
            context,
            login,
            server,
        }
    }
    #[cfg(test)]
    async fn verify(&self) -> Result<RelayCaller, StatusCode> {
        RelayCaller::verify(
            self.auth.clone(),
            &self.headers,
            &self.context,
            Some("test-access"),
            None,
            None,
        )
        .await
    }

    #[cfg(test)]
    async fn capture(&self, uri: &str) -> BrowserRelayProof {
        use axum::{body::Body, extract::Request, Extension};
        use tower::ServiceExt;
        let app = Router::new()
            .route(
                "/api/v1/agents/{id}/files/content",
                get(
                    |Extension(proof): Extension<BrowserRelayProof>| async move {
                        let mut response = axum::response::Response::new(Body::empty());
                        response.extensions_mut().insert(proof);
                        response
                    },
                ),
            )
            .layer(axum::middleware::from_fn_with_state(
                self.auth.clone(),
                super::super::attach_subject,
            ))
            .layer(Extension(ReuseFilePreviewIdentity));
        let mut request = Request::builder().uri(uri).body(Body::empty()).unwrap();
        *request.headers_mut() = self.headers.clone();
        let mut response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        response.extensions_mut().remove().unwrap()
    }

    #[cfg(test)]
    async fn verify_captured(&self, proof: &BrowserRelayProof) -> Result<RelayCaller, StatusCode> {
        RelayCaller::verify(
            self.auth.clone(),
            &self.headers,
            &self.context,
            Some("test-access"),
            Some(proof),
            None,
        )
        .await
    }
}

#[cfg(test)]
const PREVIEW: &str = "/api/v1/agents/fixture/files/content?path=sample";

#[tokio::test]
async fn peer_preview_checks_local_custody_without_provider_io() {
    let f = Fixture::new(true).await;
    let proof = f.capture(PREVIEW).await;
    let uri = PREVIEW.parse().unwrap();
    let claim = proof.peer_claim(&f.auth, &Method::GET, &uri).await.unwrap();
    let before = f.provider_calls();
    let (_, receipt) = claim
        .clone()
        .resolve(&f.auth, &f.headers, &Method::GET, &uri)
        .await
        .unwrap();
    assert!(receipt.matches(&Method::GET, &uri));
    assert!(receipt.current(&f.auth).await.is_ok());
    assert!(matches!(
        claim
            .clone()
            .resolve(&f.auth, &f.headers, &Method::HEAD, &uri)
            .await,
        Err(StatusCode::UNAUTHORIZED)
    ));
    let other = "/api/v1/agents/fixture/files/content?path=other"
        .parse()
        .unwrap();
    assert!(matches!(
        claim
            .clone()
            .resolve(&f.auth, &f.headers, &Method::GET, &other)
            .await,
        Err(StatusCode::UNAUTHORIZED)
    ));
    let mut state = f
        .store
        .subject(&f.context.caller.subject)
        .await
        .unwrap()
        .unwrap();
    state.revision += 1;
    state.token_generation += 1;
    f.store.put_subject(state).await.unwrap();
    assert_eq!(
        receipt.current(&f.auth).await,
        Err(StatusCode::UNAUTHORIZED)
    );
    assert!(matches!(
        claim.resolve(&f.auth, &f.headers, &Method::GET, &uri).await,
        Err(StatusCode::UNAUTHORIZED)
    ));
    assert_eq!(f.provider_calls(), before);
}

#[tokio::test]
async fn peer_preview_refuses_logout_deadline_and_source_epoch_change() {
    let f = Fixture::new(true).await;
    let uri = PREVIEW.parse().unwrap();
    let proof = f.capture(PREVIEW).await;
    let claim = proof.peer_claim(&f.auth, &Method::GET, &uri).await.unwrap();
    let (_, receipt) = claim
        .clone()
        .resolve(&f.auth, &f.headers, &Method::GET, &uri)
        .await
        .unwrap();
    f.auth.identity_store.invalidate_token("test-access");
    assert!(matches!(
        proof.peer_claim(&f.auth, &Method::GET, &uri).await,
        Err(StatusCode::UNAUTHORIZED)
    ));
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(10)).await;
    assert_eq!(
        receipt.current(&f.auth).await,
        Err(StatusCode::UNAUTHORIZED)
    );
    tokio::time::resume();
    f.logout().await;
    assert!(matches!(
        claim.resolve(&f.auth, &f.headers, &Method::GET, &uri).await,
        Err(StatusCode::UNAUTHORIZED)
    ));
}

#[tokio::test]
async fn file_preview_proof_reuses_one_verification_and_is_bound_to_the_request_target() {
    let f = Fixture::new(true).await;
    let before = f.provider_calls();
    let captured = f.capture(PREVIEW).await;
    assert_eq!(f.provider_calls(), (before.0 + 1, before.1 + 1));
    let uri = PREVIEW.parse().unwrap();
    assert!(captured.matches(&Method::GET, &uri));
    assert!(!captured.matches(&Method::HEAD, &uri));
    assert!(!captured.matches(
        &Method::GET,
        &"/api/v1/agents/fixture/files/content?path=other"
            .parse()
            .unwrap()
    ));
    let before = f.provider_calls();
    let admitted = f.verify_captured(&captured).await.unwrap();
    admitted.current().await.unwrap();
    assert_eq!(f.provider_calls(), before);
    // A route without the ingress extension still performs its own provider verification.
    f.verify().await.unwrap();
    assert_eq!(f.provider_calls(), (before.0 + 1, before.1 + 1));
}

#[tokio::test]
async fn file_preview_proof_refuses_logout_and_observed_revocation_across_policy_io() {
    let f = Fixture::new(true).await;
    let captured = f.capture(PREVIEW).await;
    let admitted = f.verify_captured(&captured).await.unwrap();
    f.auth.identity_store.invalidate_token("test-access");
    assert_eq!(admitted.current().await, Err(StatusCode::UNAUTHORIZED));
    assert!(matches!(
        f.verify_captured(&captured).await,
        Err(StatusCode::UNAUTHORIZED)
    ));
    let captured = f.capture(PREVIEW).await;
    let admitted = f.verify_captured(&captured).await.unwrap();
    f.logout().await;
    assert_eq!(admitted.current().await, Err(StatusCode::UNAUTHORIZED));
    assert!(matches!(
        f.verify_captured(&captured).await,
        Err(StatusCode::UNAUTHORIZED)
    ));
}

#[tokio::test]
async fn file_preview_proof_refuses_new_generation_with_identical_token_text() {
    let f = Fixture::new(true).await;
    let captured = f.capture(PREVIEW).await;
    let admitted = f.verify_captured(&captured).await.unwrap();
    let mut replacement = f
        .store
        .subject(&f.context.caller.subject)
        .await
        .unwrap()
        .unwrap();
    replacement.revision += 1;
    replacement.token_generation += 1;
    f.store.put_subject(replacement).await.unwrap();
    assert_eq!(admitted.current().await, Err(StatusCode::UNAUTHORIZED));
    assert!(matches!(
        f.verify_captured(&captured).await,
        Err(StatusCode::UNAUTHORIZED)
    ));
}

#[tokio::test]
async fn file_preview_proof_refuses_a_recreated_subject() {
    let f = Fixture::new(true).await;
    let captured = f.capture(PREVIEW).await;
    let admitted = f.verify_captured(&captured).await.unwrap();
    let mut replacement = f
        .store
        .subject(&f.context.caller.subject)
        .await
        .unwrap()
        .unwrap();
    replacement.incarnation = "recreated-subject".into();
    f.store.put_subject(replacement).await.unwrap();
    assert_eq!(admitted.current().await, Err(StatusCode::UNAUTHORIZED));
    assert!(matches!(
        f.verify_captured(&captured).await,
        Err(StatusCode::UNAUTHORIZED)
    ));
}

#[tokio::test]
async fn file_preview_proof_keeps_the_original_monotonic_deadline() {
    let f = Fixture::new(true).await;
    let captured = f.capture(PREVIEW).await;
    let admitted = f.verify_captured(&captured).await.unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(3601)).await;
    assert_eq!(admitted.current().await, Err(StatusCode::UNAUTHORIZED));
    assert!(matches!(
        f.verify_captured(&captured).await,
        Err(StatusCode::UNAUTHORIZED)
    ));
}

#[tokio::test]
async fn file_preview_proof_reports_an_unavailable_custody_store() {
    let f = Fixture::new(true).await;
    let captured = f.capture(PREVIEW).await;
    let admitted = f.verify_captured(&captured).await.unwrap();
    f.store.set_unavailable(true);
    assert_eq!(
        admitted.current().await,
        Err(StatusCode::SERVICE_UNAVAILABLE)
    );
    assert!(matches!(
        f.verify_captured(&captured).await,
        Err(StatusCode::SERVICE_UNAVAILABLE)
    ));
}

#[tokio::test]
async fn relay_browser_logout_during_provider_verification_is_refused() {
    let f = Fixture::new(true).await;
    f.provider.block.store(true, Ordering::SeqCst);
    let result = tokio::join!(f.verify(), async {
        f.provider.entered.notified().await;
        f.store.delete_browser_login(&f.login).await.unwrap();
        f.provider.release.notify_one();
    });
    assert!(matches!(result.0, Err(StatusCode::UNAUTHORIZED)));
}

#[tokio::test]
async fn relay_policy_io_cannot_adopt_replaced_browser_credential() {
    let f = Fixture::new(true).await;
    let proof = f.verify().await.unwrap();
    f.store
        .put_subject(SubjectAuthState::new(
            SubjectId::new("owner"),
            Authorization::none(),
            Profile::default(),
            Some(DurableSubjectTokens {
                access_token: SecretValue::new("replacement".into()),
                refresh_token: None,
                expires_at: SystemTime::now() + Duration::from_secs(3600),
            }),
        ))
        .await
        .unwrap();
    assert_eq!(proof.current().await, Err(StatusCode::UNAUTHORIZED));
    assert!(matches!(f.verify().await, Err(StatusCode::UNAUTHORIZED)));
}

#[tokio::test]
async fn relay_bearer_rechecks_provider_and_rejects_revocation_without_cached_success() {
    let f = Fixture::new(false).await;
    assert!(f.verify().await.is_ok());
    f.provider.denied.store(true, Ordering::SeqCst);
    assert!(f.verify().await.is_err());
}
