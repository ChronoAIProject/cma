use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use axum::{body::Body, http::Request, routing::post, Extension, Json, Router};
use layer1_public::rest::creation_outcomes::LOOKUP_PATH;
use layer2_core::account_deletion::{AccountDeletionError, AccountStanding};
use serde_json::json;
use tower::ServiceExt;

use super::*;
use crate::auth::{middleware, persistence, profile::Profile};

#[derive(Default)]
struct Standing {
    erasing: AtomicBool,
    reads: AtomicUsize,
    final_read: Option<StandingGate>,
}

#[derive(Default)]
struct StandingGate {
    reached: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl AccountStanding for Standing {
    async fn erasing(&self, _: &SubjectId) -> Result<bool, AccountDeletionError> {
        let read = self.reads.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = self.final_read.as_ref().filter(|_| read == 1) {
            gate.reached.notify_one();
            gate.release.notified().await;
        }
        Ok(self.erasing.load(Ordering::SeqCst))
    }
}

struct Provider {
    revoked: AtomicBool,
    reduced: AtomicBool,
    server: tokio::task::AbortHandle,
}

impl Drop for Provider {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn owner_auth(standing: Arc<Standing>) -> (Arc<AuthState>, SubjectId, Arc<Provider>) {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (sender, receiver) = tokio::sync::oneshot::channel::<Router>();
    let task = tokio::spawn(async move {
        axum::serve(listener, receiver.await.unwrap())
            .await
            .unwrap()
    });
    let provider = Arc::new(Provider {
        revoked: AtomicBool::new(false),
        reduced: AtomicBool::new(false),
        server: task.abort_handle(),
    });
    let configuration = json!({
        "authorization_endpoint": format!("{base}/authorize"), "token_endpoint": format!("{base}/token"),
        "userinfo_endpoint": format!("{base}/userinfo"), "introspection_endpoint": format!("{base}/introspect"),
    });
    let state = Arc::downgrade(&provider);
    sender.send(Router::new()
        .route("/.well-known/openid-configuration", axum::routing::get(move || async move { Json(configuration) }))
        .route("/userinfo", axum::routing::get(|| async { Json(json!({"sub":"fixture-owner", "permissions":["cma:sandbox:use"]})) }))
        .route("/introspect", post(move || async move {
            let provider = state.upgrade().unwrap();
            let permissions = if provider.reduced.load(Ordering::SeqCst) { vec![] } else { vec!["cma:sandbox:use"] };
            Json(json!({"active":!provider.revoked.load(Ordering::SeqCst), "sub":"fixture-owner", "permissions":permissions}))
        }))).unwrap();
    let owner = SubjectId::new("fixture-owner");
    let store = Arc::new(persistence::MemoryAuthStore::default());
    let mode = AuthMode::Provider {
        base_url: base,
        client_id: "fixture-app".into(),
        client_secret: String::new(),
        public_url: "https://cma.example.test".into(),
    };
    store
        .put_subject(persistence::SubjectAuthState::new(
            owner.clone(),
            Authorization::new([Capability::UseOwnSandboxes]),
            Profile::default(),
            Some(persistence::DurableSubjectTokens {
                access_token: persistence::SecretValue::new(uuid::Uuid::new_v4().to_string()),
                refresh_token: None,
                expires_at: SystemTime::now() + Duration::from_secs(600),
            }),
        ))
        .await
        .unwrap();
    let auth = AuthState::with_store(mode, None, store)
        .with_account_standing(standing)
        .with_creation_outcomes_enabled(true);
    (auth, owner, provider)
}

fn router(auth: Arc<AuthState>, standing: Arc<Standing>) -> Router {
    let store = auth.store.clone();
    Router::new()
        .route(
            LOOKUP_PATH,
            post(
                move |Extension(owner): Extension<AuthorizedLookup>, request: Request<Body>| {
                    let standing = standing.clone();
                    let store = store.clone();
                    async move {
                        if request.headers().contains_key("x-fixture-erase") {
                            standing.erasing.store(true, Ordering::SeqCst);
                        }
                        if request.headers().contains_key("x-fixture-rotate") {
                            let mut replacement = store.subject(&owner.0).await.unwrap().unwrap();
                            let revision = replacement.revision;
                            let incarnation = replacement.incarnation.clone();
                            replacement.revision += 1;
                            replacement.token_generation += 1;
                            replacement.tokens.as_mut().unwrap().access_token =
                                persistence::SecretValue::new(uuid::Uuid::new_v4().to_string());
                            assert!(store
                                .replace_subject(&incarnation, revision, replacement)
                                .await
                                .unwrap());
                        }
                        assert_eq!(owner.0, SubjectId::new("fixture-owner"));
                        Json(json!({"proof_id":"fixture-proof"}))
                    }
                },
            ),
        )
        .layer(axum::middleware::from_fn_with_state(
            auth,
            middleware::attach_subject,
        ))
}

#[tokio::test]
async fn browser_owner_is_required_at_entry_and_before_releasing_evidence() {
    let standing = Arc::new(Standing::default());
    let (auth, owner, _provider) = owner_auth(standing.clone()).await;
    let session = auth.issue_browser_login(owner).await.unwrap();
    let app = router(auth.clone(), standing.clone());
    let request = || Request::post(LOOKUP_PATH);
    let cookie = || format!("{AUTH_COOKIE}={session}");
    for (request, expected) in [
        (request(), StatusCode::UNAUTHORIZED),
        (request().header("cookie", cookie()), StatusCode::OK),
        (
            request()
                .header("cookie", cookie())
                .header("x-fixture-erase", "true"),
            StatusCode::FORBIDDEN,
        ),
        (request().header("cookie", cookie()), StatusCode::FORBIDDEN),
    ] {
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    standing.erasing.store(false, Ordering::SeqCst);
    auth.revoke_browser_login(&session).await.unwrap();
    let response = app
        .oneshot(
            request()
                .header("cookie", cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn signed_proxy_read_keeps_the_grant_ceiling_and_accepts_fresh_assertions() {
    let standing = Arc::new(Standing::default());
    let (mut auth, _, provider) = owner_auth(standing.clone()).await;
    let signer = jwt::Signer::primary();
    let jwks = json!({"keys":[signer.key("fixture-key")]});
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let jwks = Router::new().route(
            "/jwks",
            axum::routing::get(move || async move { Json(jwks) }),
        );
        axum::serve(listener, jwks).await.unwrap();
    });
    let issuer = "https://nyx.example.test";
    let audience = "https://cma.example.test";
    let service = "fixture-cma-service";
    Arc::get_mut(&mut auth).unwrap().identity_proxy = Some(
        jwt::Verifier::new(jwt::Config {
            issuer: issuer.into(),
            audience: audience.into(),
            service_id: service.into(),
            jwks_url: format!("http://{address}/jwks"),
        })
        .unwrap(),
    );
    let app = router(auth, standing);
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    for (jti, aud, service_id, permissions, expected) in [
        (
            "first",
            audience,
            service,
            vec!["cma:sandbox:use"],
            StatusCode::OK,
        ),
        (
            "second",
            audience,
            service,
            vec!["cma:sandbox:use"],
            StatusCode::OK,
        ),
        ("narrow", audience, service, vec![], StatusCode::FORBIDDEN),
        (
            "audience",
            issuer,
            service,
            vec!["cma:sandbox:use"],
            StatusCode::UNAUTHORIZED,
        ),
        (
            "service",
            audience,
            "other",
            vec!["cma:sandbox:use"],
            StatusCode::UNAUTHORIZED,
        ),
    ] {
        let claims = json!({"sub":"fixture-owner","iss":issuer,"aud":aud,"jti":jti,
            "iat":now,"exp":now+60,"nyx_service_id":service_id,"permissions":permissions});
        let request = Request::post(LOOKUP_PATH)
            .header(jwt::HEADER, signer.token(&claims, "fixture-key"))
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), expected, "{jti}");
    }
    provider.reduced.store(true, Ordering::SeqCst);
    let claims = json!({"sub":"fixture-owner","iss":issuer,"aud":audience,"jti":"reduced-owner",
        "iat":now,"exp":now+60,"nyx_service_id":service,"permissions":["cma:sandbox:use"]});
    let request = Request::post(LOOKUP_PATH)
        .header(jwt::HEADER, signer.token(&claims, "fixture-key"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.oneshot(request).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    server.abort();
}

#[tokio::test]
async fn receipt_requires_live_provider_permissions_and_the_same_credential_at_release() {
    let standing = Arc::new(Standing::default());
    let (auth, owner, provider) = owner_auth(standing.clone()).await;
    let session = auth.issue_browser_login(owner).await.unwrap();
    let app = router(auth, standing);
    let request =
        || Request::post(LOOKUP_PATH).header("cookie", format!("{AUTH_COOKIE}={session}"));
    provider.revoked.store(true, Ordering::SeqCst);
    let response = app
        .clone()
        .oneshot(request().body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    provider.revoked.store(false, Ordering::SeqCst);
    provider.reduced.store(true, Ordering::SeqCst);
    let response = app
        .clone()
        .oneshot(request().body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    provider.reduced.store(false, Ordering::SeqCst);
    let response = app
        .clone()
        .oneshot(
            request()
                .header("x-fixture-rotate", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let response = app
        .oneshot(request().body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn invalidated_provider_receipt_prevents_release_during_final_standing_read() {
    let standing = Arc::new(Standing {
        final_read: Some(StandingGate::default()),
        ..Standing::default()
    });
    let (auth, owner, _provider) = owner_auth(standing.clone()).await;
    let session = auth.issue_browser_login(owner.clone()).await.unwrap();
    let saved = auth.store.subject(&owner).await.unwrap().unwrap();
    let app = router(auth.clone(), standing.clone());
    let request = Request::post(LOOKUP_PATH)
        .header("cookie", format!("{AUTH_COOKIE}={session}"))
        .body(Body::empty())
        .unwrap();
    let lookup = tokio::spawn(app.oneshot(request));
    let gate = standing.final_read.as_ref().unwrap();
    tokio::time::timeout(Duration::from_secs(2), gate.reached.notified())
        .await
        .expect("the final verification reached the standing read");

    auth.identity_store
        .invalidate_token(saved.tokens.as_ref().unwrap().access_token.expose());
    gate.release.notify_one();

    let response = lookup.await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(response.into_body(), 2048)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(body.get("proof_id").is_none());
    cookie_authorization::require_current_custody(&auth, &saved)
        .await
        .unwrap();
    assert!(auth
        .store
        .browser_login(&persistence::SecretDigest::identify(
            AUTH_COOKIE_DOMAIN,
            &session
        ))
        .await
        .unwrap()
        .is_some());
}
