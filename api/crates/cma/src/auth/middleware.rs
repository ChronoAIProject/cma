//! Attach verified identity and authority to one incoming request.

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use layer1_public::{AuthenticatedSubject, DelegatedCredential};

use super::AuthState;
use layer2_core::dependency_diagnostics::{
    self as diagnostics, Dependency, Failure, Operation, Reason,
};

pub async fn attach_subject(
    State(auth): State<Arc<AuthState>>,
    mut request: axum::extract::Request,
    next: Next,
) -> Response {
    if super::worker_credentials::matches_path(request.uri().path()) {
        return super::worker_credentials::attach(&auth, request, next).await;
    }
    if request
        .headers()
        .contains_key(super::resource_request::HEADER)
    {
        return super::resource_request::attach(&auth, request, next).await;
    }
    if bypasses_ambient_auth(request.uri().path()) {
        return next.run(request).await;
    }
    let allow_fallback = auth
        .fallback_policy
        .permits(request.method(), request.uri().path());
    let started = std::time::Instant::now();
    let (resolved, failure) =
        diagnostics::capture(auth.resolve_request_with_policy(request.headers(), allow_fallback))
            .await;
    let outcome = match &resolved {
        Ok(Some(_)) => "resolved",
        Ok(None) => "anonymous",
        Err(_) => "unavailable",
    };
    metrics::counter!("cma_auth_resolutions_total", "outcome" => outcome).increment(1);
    metrics::histogram!("cma_auth_resolution_seconds", "outcome" => outcome)
        .record(started.elapsed().as_secs_f64());
    let resolved = match resolved {
        Ok(resolved) => resolved,
        Err(error) => {
            diagnostics::report(
                Operation::Authentication,
                failure.or(Some(Failure::new(
                    Dependency::AuthenticationAuthority,
                    Reason::Unknown,
                ))),
            );
            if request.uri().path() == "/auth/session"
                && matches!(&error, layer2_core::StoreError::Unavailable(detail) if detail == "identity_provider_unavailable")
            {
                return identity_failure();
            }
            return store_failure();
        }
    };
    if let Some(resolved) = resolved {
        if request.uri().path() == "/api/v1/bots"
            || request.uri().path().starts_with("/api/v1/bots/")
        {
            match auth.authorize_bot_request(&resolved).await {
                Ok(true) => {},
                Ok(false) => return (
                    StatusCode::FORBIDDEN,
                    [(axum::http::header::CONTENT_TYPE, "application/problem+json")],
                    Json(serde_json::json!({"type":"about:blank","title":"renewable owner authority required","status":403,
                        "detail":"Sign in to CMA with this account to authorize renewable Bot credentials.","code":"bot_owner_authority_required"})),
                ).into_response(),
                Err(_) => return store_failure(),
            }
        }
        if let Some(binding) = &resolved.binding {
            if !super::binding::permits_request(
                request.method(),
                request.uri().path(),
                binding.binding_id(),
            ) {
                return (StatusCode::FORBIDDEN, Json(serde_json::json!({"error":{"code":"binding_key_scope","message":"Binding keys are limited to their conversation interface.","type":"permission_error"}}))).into_response();
            }
            request
                .extensions_mut()
                .insert(layer1_public::AuthMethod::BindingKey {
                    binding_id: binding.binding_id().into(),
                });
        }
        request
            .extensions_mut()
            .insert(auth.resource_request_context(&resolved));
        if let Some(value) = resolved.delegated_credential {
            request
                .extensions_mut()
                .insert(DelegatedCredential::new(value));
        }
        request.extensions_mut().insert(resolved.authorization);
        request.extensions_mut().insert(resolved.profile);
        request
            .extensions_mut()
            .insert(AuthenticatedSubject(resolved.subject));
    }
    next.run(request).await
}

pub(super) fn identity_failure() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [
            (axum::http::header::CONTENT_TYPE, "application/problem+json"),
            (axum::http::header::RETRY_AFTER, "3"),
            (axum::http::header::CACHE_CONTROL, "no-store"),
        ],
        Json(serde_json::json!({
            "type": "https://spec.chrono-ai.fun/cma/1.0/problems/identity-unavailable",
            "title": "Identity service temporarily unavailable",
            "status": 503,
            "detail": "The identity service is temporarily unavailable. Please retry shortly.",
            "code": "identity_unavailable",
        })),
    )
        .into_response()
}

pub(super) fn store_failure() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "type": "https://spec.chrono-ai.fun/cma/1.0/problems/store-failed",
            "title": "store failed",
            "status": 503,
            "detail": "Authentication authority is temporarily unavailable.",
            "code": "store_failed",
        })),
    )
        .into_response()
}

fn bypasses_ambient_auth(path: &str) -> bool {
    // Recovery routes validate their own authority without requiring an old login.
    if matches!(
        path,
        "/auth/signup-options"
            | "/auth/login"
            | "/auth/callback"
            | "/auth/logout"
            | "/.well-known/agent-card.json"
    ) {
        return true;
    }
    let Some(path) = path.strip_prefix('/') else {
        return false;
    };
    let entry = path.split('/').collect::<Vec<_>>();
    if matches!(entry.as_slice(),
        ["api", "v1", "triggers", "public", public_id, "entry-intent"]
            if !public_id.is_empty())
        || matches!(entry.as_slice(),
            ["api", "v1", "triggers", "public", public_id, "open", placement_id]
                if !public_id.is_empty() && !placement_id.is_empty())
    {
        return true;
    }
    let mut segments = path.split('/');
    matches!(
        (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
        ),
        (
            Some("api"),
            Some("v1"),
            Some("triggers"),
            Some("public"),
            Some(public_id),
            None,
            None,
        ) | (
            Some("api"),
            Some("v1"),
            Some("triggers"),
            Some("public"),
            Some(public_id),
            Some("badge.svg"),
            None,
        ) if !public_id.is_empty()
    )
}

#[cfg(test)]
mod tests {
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
            .layer(axum::middleware::from_fn_with_state(auth, attach_subject));
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
}
