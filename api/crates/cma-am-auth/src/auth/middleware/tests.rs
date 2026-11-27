use axum::{
    body::Body,
    extract::Extension,
    http::{header, Request},
    routing::get,
    Router,
};
use layer2_core::Authorization;
use tower::ServiceExt as _;

use super::*;
use crate::auth::{persistence::MemoryAuthStore, AuthMode};

#[test]
fn only_explicit_routes_bypass_ambient_auth() {
    for path in [
        "/.well-known/agent-card.json",
        "/auth/signup-options",
        "/auth/login",
        "/auth/callback",
        "/auth/logout",
        "/api/v1/triggers/public/pub_example",
        "/api/v1/triggers/public/pub_example/badge.svg",
        "/api/v1/triggers/public/pub_example/open/plc_readme",
        "/api/v1/triggers/public/pub_example/entry-intent",
    ] {
        assert!(bypasses_ambient_auth(path), "expected bypass for {path}");
    }

    for path in [
        "/auth/session",
        "/auth/login/",
        "/auth/callback/extra",
        "/auth/logout/extra",
        "auth/signup-options",
        "/auth/signup-options/",
        "/auth//signup-options",
        "/auth/signup-options/extra",
        "/api/v1/triggers/public",
        "/api/v1/triggers/public/",
        "/api/v1/triggers/public/pub_example/",
        "/api/v1/triggers/public//badge.svg",
        "/api//v1/triggers/public/pub_example",
        "/api/v1/triggers/public/pub_example/extra",
        "/api/v1/triggers/public/pub_example/badge.svg/extra",
        "/api/v1/triggers/public/pub_example/launches",
    ] {
        assert!(!bypasses_ambient_auth(path), "unexpected bypass for {path}");
    }
}

#[tokio::test]
async fn trusted_identity_is_attached_with_its_authority() {
    async fn inspect(
        Extension(subject): Extension<AuthenticatedSubject>,
        Extension(authority): Extension<Authorization>,
        Extension(context): Extension<layer2_core::resource_access::CallerContext>,
    ) -> String {
        assert!(context.validate_principal());
        assert_eq!(&context.caller.subject, subject.subject());
        assert_eq!(context.caller.authorization, authority);
        assert!(context.memberships.is_empty());
        format!("{}:{}", subject.subject(), authority.iter().count())
    }
    let auth = AuthState::in_memory(
        AuthMode::Trusted {
            subject: "developer".into(),
        },
        None,
    );
    let app = Router::new()
        .route("/", get(inspect))
        .layer(axum::middleware::from_fn_with_state(
            auth,
            |state, request, next| async move {
                let future = attach_subject(state, request, next);
                // Leave room for the request router and the nested diagnostic scopes
                // when polling this middleware on a default Tokio worker stack.
                let bytes = std::mem::size_of_val(&future);
                assert!(
                    bytes <= 64 * 1024,
                    "authentication future uses {bytes} bytes"
                );
                future.await
            },
        ));
    let response = app
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn public_preview_routes_ignore_ambient_credentials() {
    let app = public_trigger_app_with_unavailable_auth();

    for path in [
        "/api/v1/triggers/public/pub_example",
        "/api/v1/triggers/public/pub_example/badge.svg",
    ] {
        let response = app
            .clone()
            .oneshot(stale_credentials_request(path))
            .await
            .expect("router answers");
        assert_eq!(response.status(), StatusCode::OK, "{path}");
    }
}

#[tokio::test]
async fn public_launch_route_still_resolves_authentication() {
    let response = public_trigger_app_with_unavailable_auth()
        .oneshot(stale_credentials_request(
            "/api/v1/triggers/public/pub_example/launches",
        ))
        .await
        .expect("router answers");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn stale_provider_access_cannot_block_sign_in_callback_or_logout() {
    use std::time::{Duration, SystemTime};

    use crate::auth::persistence::{
        AuthStore as _, BrowserLogin, DurableSubjectTokens, SecretDigest, SecretValue,
        SubjectAuthState,
    };
    use crate::auth::{profile::Profile, ProviderMetadata};
    use layer2_core::SubjectId;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let upstream = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/userinfo", get(|| async { StatusCode::UNAUTHORIZED })),
        )
        .await
        .unwrap();
    });
    let store = Arc::new(MemoryAuthStore::default());
    let owner = SubjectId::new("recovery-owner");
    let cookie_id = SecretDigest::identify("browser-login", "stale");
    store
        .put_browser_login(BrowserLogin {
            id: cookie_id.clone(),
            subject: owner.clone(),
            expires_at: SystemTime::now() + Duration::from_secs(3600),
        })
        .await
        .unwrap();
    store
        .put_subject(SubjectAuthState::new(
            owner,
            Authorization::none(),
            Profile::default(),
            Some(DurableSubjectTokens {
                access_token: SecretValue::new("revoked-fixture-access".into()),
                refresh_token: None,
                expires_at: SystemTime::now() + Duration::from_secs(3600),
            }),
        ))
        .await
        .unwrap();
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
    let app = crate::auth::router(auth.clone())
        .layer(axum::middleware::from_fn_with_state(auth, attach_subject));
    for (path, expected) in [
        ("/auth/session", StatusCode::UNAUTHORIZED),
        ("/auth/login", StatusCode::SEE_OTHER),
        ("/auth/callback", StatusCode::BAD_REQUEST),
        ("/auth/logout", StatusCode::SEE_OTHER),
    ] {
        let response = app
            .clone()
            .oneshot(stale_credentials_request(path))
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{path}");
    }
    assert!(store.browser_login(&cookie_id).await.unwrap().is_none());
    upstream.abort();
}

fn public_trigger_app_with_unavailable_auth() -> Router {
    let store = Arc::new(MemoryAuthStore::default());
    store.set_unavailable(true);
    let auth = AuthState::with_store(
        AuthMode::Provider {
            base_url: "http://identity.invalid".into(),
            client_id: "client".into(),
            client_secret: "secret".into(),
            public_url: "http://cma.invalid".into(),
        },
        None,
        store,
    );
    Router::new()
        .route(
            "/api/v1/triggers/public/{public_id}",
            get(public_trigger_handler),
        )
        .route(
            "/api/v1/triggers/public/{public_id}/badge.svg",
            get(public_trigger_handler),
        )
        .route(
            "/api/v1/triggers/public/{public_id}/launches",
            get(public_trigger_handler),
        )
        .layer(axum::middleware::from_fn_with_state(auth, attach_subject))
}

async fn public_trigger_handler() -> StatusCode {
    StatusCode::OK
}

fn stale_credentials_request(path: &str) -> Request<Body> {
    Request::get(path)
        .header(header::COOKIE, "cma_session=stale")
        .header(header::AUTHORIZATION, "Bearer stale")
        .body(Body::empty())
        .expect("request builds")
}
