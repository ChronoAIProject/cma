use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::{body::Body, http::Request, routing::post, Extension, Json, Router};
use layer1_public::rest::creation_outcomes::LOOKUP_PATH;
use layer2_core::account_deletion::{AccountDeletionError, AccountStanding};
use serde_json::json;
use tower::ServiceExt;

use super::*;
use crate::auth::{middleware, persistence, profile::Profile};

#[derive(Default)]
struct Standing(AtomicBool);

#[async_trait::async_trait]
impl AccountStanding for Standing {
    async fn erasing(&self, _: &SubjectId) -> Result<bool, AccountDeletionError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

async fn owner_auth(standing: Arc<Standing>) -> (Arc<AuthState>, SubjectId) {
    let owner = SubjectId::new("fixture-owner");
    let store = Arc::new(persistence::MemoryAuthStore::default());
    let mode = AuthMode::Provider {
        base_url: "https://nyx.example.test".into(),
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
    (auth, owner)
}

fn router(auth: Arc<AuthState>, standing: Arc<Standing>) -> Router {
    Router::new()
        .route(
            LOOKUP_PATH,
            post(
                move |Extension(owner): Extension<AuthorizedLookup>, request: Request<Body>| {
                    let standing = standing.clone();
                    async move {
                        if request.headers().contains_key("x-fixture-erase") {
                            standing.0.store(true, Ordering::SeqCst);
                        }
                        assert_eq!(owner.0, SubjectId::new("fixture-owner"));
                        StatusCode::OK
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
    let (auth, owner) = owner_auth(standing.clone()).await;
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
    standing.0.store(false, Ordering::SeqCst);
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
    let (mut auth, _) = owner_auth(standing.clone()).await;
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
    server.abort();
}
