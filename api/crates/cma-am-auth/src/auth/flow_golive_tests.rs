//! Public browser route proofs: no aggregate creation, exact custody and logout races.
use super::*;
use crate::auth::{
    persistence::{AuthStore, MemoryAuthStore},
    ProviderMetadata,
};
use axum::{
    body::{to_bytes, Body},
    http::Request,
    routing::post,
};
use layer2_core::SubjectId;
use serde_json::json;
use tokio::sync::Notify;
use tower::ServiceExt;

struct Fixture {
    app: Router,
    second: Router,
    store: Arc<MemoryAuthStore>,
    reached: Arc<Notify>,
    release: Arc<Notify>,
    server: tokio::task::JoinHandle<()>,
    auth: Arc<AuthState>,
    identity_status: Arc<std::sync::atomic::AtomicU16>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn new(token_status: StatusCode, gate: bool) -> Self {
        let reached = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let identity_status = Arc::new(std::sync::atomic::AtomicU16::new(200));
        let identity_reply = identity_status.clone();
        let entered = reached.clone();
        let resume = release.clone();
        let upstream = Router::new()
            .route(
                "/token",
                post(move || {
                    let entered = entered.clone();
                    let resume = resume.clone();
                    async move {
                        entered.notify_one();
                        if gate {
                            resume.notified().await;
                        }
                        (
                            token_status,
                            Json(json!({"access_token":"test-browser-access", "expires_in":3600})),
                        )
                    }
                }),
            )
            .route(
                "/userinfo",
                get(move || {
                    let status = identity_reply.load(std::sync::atomic::Ordering::SeqCst);
                    async move {
                        (
                            StatusCode::from_u16(status).unwrap(),
                            Json(
                                json!({"sub":"returning-owner", "permissions":["cma:sandbox:use"]}),
                            ),
                        )
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let store = Arc::new(MemoryAuthStore::default());
        let mut apps = Vec::new();
        let mut states = Vec::new();
        for _ in 0..2 {
            let auth = AuthState::with_store(
                AuthMode::Provider {
                    base_url: base.clone(),
                    client_id: "fixture-client".into(),
                    client_secret: "fixture-value".into(),
                    public_url: "https://cma.example".into(),
                },
                None,
                store.clone(),
            );
            *auth.metadata.write().await = Some(ProviderMetadata {
                authorization_endpoint: format!("{base}/authorize"),
                token_endpoint: format!("{base}/token"),
                userinfo_endpoint: Some(format!("{base}/userinfo")),
                introspection_endpoint: None,
            });
            states.push(auth.clone());
            apps.push(router(auth));
        }
        Self {
            app: apps.remove(0),
            second: apps.remove(0),
            store,
            reached,
            release,
            server,
            auth: states.remove(0),
            identity_status,
        }
    }
    async fn start(&self, silent: bool, target: &str) -> (String, String) {
        let response = call(
            &self.app,
            &format!(
                "/auth/login?return_to={}{}",
                urlencode(target),
                if silent { "&prompt=none" } else { "" }
            ),
            "",
        )
        .await;
        let url =
            reqwest::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        let state = url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        (
            format!("/auth/callback?state={state}&code=test-code"),
            cookies(&response)[0].split(';').next().unwrap().to_owned(),
        )
    }
}
async fn call(app: &Router, path: &str, cookie: &str) -> Response {
    app.clone()
        .oneshot(
            Request::get(path)
                .header("sec-fetch-site", "same-origin")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}
fn cookies(response: &Response) -> Vec<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn successful_silent_callback_uses_bound_custody_once_and_restores_query() {
    let f = Fixture::new(StatusCode::OK, false).await;
    let (path, cookie) = f.start(true, "/sandboxes/sbx_test?tab=access").await;
    let binding = cookie.split_once('=').unwrap().1;
    let id = SecretDigest::identify(AUTH_COOKIE_DOMAIN, binding);
    assert!(
        f.store.browser_login(&id).await.unwrap().is_none(),
        "pending custody is not identity"
    );
    let response = call(&f.second, &path, &cookie).await;
    assert_eq!(
        response.headers()[header::LOCATION],
        "/sandboxes/sbx_test?tab=access"
    );
    let values = cookies(&response);
    let session = values[0]
        .split(';')
        .next()
        .unwrap()
        .split_once('=')
        .unwrap()
        .1;
    assert_ne!(
        session, binding,
        "a planted binding must never become a session token"
    );
    let session_id = SecretDigest::identify(AUTH_COOKIE_DOMAIN, session);
    assert!(f.store.browser_login(&id).await.unwrap().is_none());
    assert!(values[1].contains("Max-Age=0"));
    assert!(values
        .iter()
        .all(|value| value.contains("HttpOnly; Secure; SameSite=Lax")));
    assert_eq!(
        f.store
            .browser_login(&session_id)
            .await
            .unwrap()
            .unwrap()
            .subject
            .as_str(),
        "returning-owner"
    );
    let replay = call(&f.app, &path, &cookie).await;
    assert_eq!(replay.status(), StatusCode::SEE_OTHER);
    assert!(
        cookies(&replay).is_empty(),
        "replay must retain the original callback's logout handle"
    );
}

#[tokio::test]
async fn logout_fences_processing_callbacks_on_another_replica_even_after_replay() {
    for silent in [false, true] {
        let f = Fixture::new(StatusCode::OK, true).await;
        let (path, cookie) = f.start(silent, "/agents/agt_test").await;
        let pending = {
            let app = f.app.clone();
            let path = path.clone();
            let cookie = cookie.clone();
            tokio::spawn(async move { call(&app, &path, &cookie).await })
        };
        tokio::time::timeout(Duration::from_secs(3), f.reached.notified())
            .await
            .unwrap();
        let replay = call(&f.second, &path, &cookie).await;
        assert_eq!(
            replay.status(),
            if silent {
                StatusCode::SEE_OTHER
            } else {
                StatusCode::BAD_REQUEST
            }
        );
        assert!(cookies(&replay).is_empty());
        let logout = call(&f.second, "/auth/logout", &cookie).await;
        assert_eq!(logout.status(), StatusCode::SEE_OTHER);
        assert!(cookies(&logout)
            .iter()
            .any(|value| value.starts_with("cma_signed_out=") && value.contains("HttpOnly")));
        f.release.notify_one();
        let response = pending.await.unwrap();
        assert!(cookies(&response)
            .iter()
            .all(|value| !value.starts_with("cma_session=")));
        if silent {
            assert_eq!(response.headers()[header::LOCATION], "/agents/agt_test");
        } else {
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
        let id = SecretDigest::identify(AUTH_COOKIE_DOMAIN, cookie.split_once('=').unwrap().1);
        assert!(f.store.browser_login(&id).await.unwrap().is_none());
    }
}

#[tokio::test]
async fn silent_exchange_failure_returns_without_authority_or_error_page() {
    for status in [
        StatusCode::BAD_REQUEST,
        StatusCode::UNAUTHORIZED,
        StatusCode::SERVICE_UNAVAILABLE,
    ] {
        let f = Fixture::new(status, false).await;
        let (path, cookie) = f.start(true, "/bots/new?setup=%7B%22v%22%3A1%7D").await;
        let response = call(&f.app, &path, &cookie).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers()[header::LOCATION],
            "/bots/new?setup=%7B%22v%22%3A1%7D"
        );
        assert!(cookies(&response)
            .iter()
            .all(|value| !value.starts_with("cma_session=")));
        assert!(to_bytes(response.into_body(), 4096)
            .await
            .unwrap()
            .is_empty());
    }
}

#[tokio::test]
async fn logout_suppresses_bare_navigation_and_keeps_marker_on_store_failure() {
    let f = Fixture::new(StatusCode::OK, false).await;
    let logout = call(&f.app, "/auth/logout", "").await;
    assert!(cookies(&logout)
        .iter()
        .any(|value| value.starts_with("cma_signed_out=")));
    let stopped = call(
        &f.app,
        "/auth/login?prompt=none&return_to=%2Fagents%2Fagt_test",
        "cma_signed_out=1",
    )
    .await;
    assert_eq!(stopped.headers()[header::LOCATION], "/agents/agt_test");
    assert!(cookies(&stopped).is_empty());
    f.store.set_unavailable(true);
    let failed = call(&f.app, "/auth/logout", "cma_session=fixture-browser").await;
    assert_eq!(failed.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(cookies(&failed)
        .iter()
        .any(|value| value.starts_with("cma_signed_out=")));
    assert!(!failed.headers().contains_key(header::LOCATION));
}

#[tokio::test]
async fn cross_site_silent_entry_never_creates_login_custody() {
    let f = Fixture::new(StatusCode::OK, false).await;
    for (name, value) in [
        ("sec-fetch-site", "cross-site"),
        ("sec-fetch-site", "same-site"),
        ("origin", "https://other.example"),
    ] {
        let response = f
            .app
            .clone()
            .oneshot(
                Request::get("/auth/login?prompt=none")
                    .header(name, value)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], "/");
        assert!(cookies(&response).is_empty());
    }
}

#[tokio::test]
async fn consent_continuation_cannot_recreate_custody_deleted_by_logout() {
    let f = Fixture::new(StatusCode::OK, false).await;
    let (path, cookie) = f.start(true, "/agent-manager").await;
    let refusal = path.replace(
        "code=test-code",
        "error=consent_required&error_description=Approve&error_uri=https%3A%2F%2Fexample.test",
    );
    let consent = call(&f.app, &refusal, &cookie).await;
    let url = reqwest::Url::parse(consent.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    assert!(url.query_pairs().all(|(key, _)| key != "prompt"));
    let next_state = url
        .query_pairs()
        .find(|(key, _)| key == "state")
        .unwrap()
        .1
        .into_owned();
    let next_cookie = cookies(&consent)[0].split(';').next().unwrap().to_owned();
    assert_eq!(
        next_cookie.split_once('=').unwrap().1,
        cookie.split_once('=').unwrap().1
    );
    call(&f.second, "/auth/logout", &cookie).await;
    let response = call(
        &f.app,
        &format!("/auth/callback?state={next_state}&code=test-code"),
        &next_cookie,
    )
    .await;
    assert_eq!(response.headers()[header::LOCATION], "/agent-manager");
    assert!(cookies(&response)
        .iter()
        .all(|value| !value.starts_with("cma_session=")));
}

#[tokio::test]
async fn failed_logout_preserves_handles_for_recovery_and_retry() {
    let f = Fixture::new(StatusCode::OK, true).await;
    let (path, cookie) = f.start(true, "/agent-manager").await;
    let pending = {
        let app = f.app.clone();
        let path = path.clone();
        let cookie = cookie.clone();
        tokio::spawn(async move { call(&app, &path, &cookie).await })
    };
    tokio::time::timeout(Duration::from_secs(3), f.reached.notified())
        .await
        .unwrap();
    f.store.set_unavailable(true);
    let failed = call(
        &f.second,
        "/auth/logout",
        &format!("{cookie}; cma_session=older-session"),
    )
    .await;
    assert_eq!(failed.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(cookies(&failed).len(), 1);
    assert!(cookies(&failed)[0].starts_with("cma_signed_out="));
    assert!(
        cookies(&failed)[0].ends_with("; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=2592000")
    );
    f.store.set_unavailable(false);
    let retried = call(&f.second, "/auth/logout", &cookie).await;
    assert_eq!(retried.status(), StatusCode::SEE_OTHER);
    f.release.notify_one();
    let response = pending.await.unwrap();
    assert!(cookies(&response)
        .iter()
        .all(|value| !value.starts_with("cma_session=")));
}

#[tokio::test]
async fn silent_discovery_and_identity_failures_never_issue_a_session() {
    let f = Fixture::new(StatusCode::OK, false).await;
    *f.auth.metadata.write().await = None;
    let response = call(
        &f.app,
        "/auth/login?prompt=none&return_to=%2Fagent-manager",
        "",
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/agent-manager");
    assert!(cookies(&response).is_empty());
    for status in [401, 403, 429, 503] {
        let f = Fixture::new(StatusCode::OK, false).await;
        let (path, cookie) = f.start(true, "/agent-manager").await;
        f.identity_status
            .store(status, std::sync::atomic::Ordering::SeqCst);
        let response = call(&f.app, &path, &cookie).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], "/agent-manager");
        assert!(cookies(&response)
            .iter()
            .all(|value| !value.starts_with("cma_session=")));
    }
}

#[tokio::test]
async fn missing_or_consumed_automatic_state_returns_home_without_authority() {
    let f = Fixture::new(StatusCode::OK, false).await;
    let (path, cookie) = f.start(true, "/agents/agt_test").await;
    let response = call(&f.app, &path, "").await;
    assert_eq!(response.headers()[header::LOCATION], "/");
    assert!(cookies(&response).is_empty());
    f.store.set_unavailable(true);
    let response = call(&f.app, &path, &cookie).await;
    assert_eq!(response.headers()[header::LOCATION], "/");
    assert!(cookies(&response).is_empty());
}

#[tokio::test]
async fn consent_cancellation_and_repeated_consent_are_bounded_silent_refusals() {
    for error in ["access_denied", "consent_required"] {
        let f = Fixture::new(StatusCode::OK, false).await;
        let (path, cookie) = f.start(true, "/agent-manager").await;
        let consent = call(
            &f.app,
            &path.replace("code=test-code", "error=consent_required"),
            &cookie,
        )
        .await;
        let url =
            reqwest::Url::parse(consent.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        let state = url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        let binding = cookies(&consent)[0].split(';').next().unwrap().to_owned();
        let response = call(
            &f.app,
            &format!("/auth/callback?state={state}&error={error}"),
            &binding,
        )
        .await;
        assert_eq!(response.headers()[header::LOCATION], "/agent-manager");
        assert!(cookies(&response)
            .iter()
            .all(|value| value.contains("Max-Age=0")));
    }
}

#[path = "flow_review_tests.rs"]
mod review_tests;

struct ErasingAccount;
#[async_trait::async_trait]
impl layer2_core::account_deletion::AccountStanding for ErasingAccount {
    async fn erasing(
        &self,
        _: &SubjectId,
    ) -> Result<bool, layer2_core::account_deletion::AccountDeletionError> {
        Ok(true)
    }
}
#[tokio::test]
async fn standing_gate_denies_login_callback_without_issuing_browser_authority() {
    let fixture = Fixture::new(StatusCode::OK, false).await;
    let (path, cookie) = fixture.start(false, "/").await;
    let auth = AuthState::with_store(fixture.auth.mode.clone(), None, fixture.store.clone())
        .with_account_standing(Arc::new(ErasingAccount));
    *auth.metadata.write().await = fixture.auth.metadata.read().await.clone();
    let response = call(&router(auth), &path, &cookie).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["code"], "account_deletion_in_progress");
    assert!(fixture
        .store
        .subject(&SubjectId::new("returning-owner"))
        .await
        .unwrap()
        .is_none());
}
