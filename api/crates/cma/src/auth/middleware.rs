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

pub async fn attach_subject(
    State(auth): State<Arc<AuthState>>,
    mut request: axum::extract::Request,
    next: Next,
) -> Response {
    if bypasses_ambient_auth(request.uri().path()) {
        return next.run(request).await;
    }
    let resolved = match auth.resolve_request(request.headers()).await {
        Ok(resolved) => resolved,
        Err(error) => {
            tracing::error!(error = %error, "authentication store lookup failed");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "type": "https://spec.chrono-ai.fun/cma/1.0/problems/store-failed",
                    "title": "store failed",
                    "status": 503,
                    "detail": "Authentication authority is temporarily unavailable.",
                    "code": "store_failed",
                })),
            )
                .into_response();
        }
    };
    if let Some(resolved) = resolved {
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

fn bypasses_ambient_auth(path: &str) -> bool {
    if path == "/auth/signup-options" {
        return true;
    }
    let Some(path) = path.strip_prefix('/') else {
        return false;
    };
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
            "/auth/signup-options",
            "/api/v1/triggers/public/pub_example",
            "/api/v1/triggers/public/pub_example/badge.svg",
        ] {
            assert!(bypasses_ambient_auth(path), "expected bypass for {path}");
        }

        for path in [
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
        ) -> String {
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
