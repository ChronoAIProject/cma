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
    // Registration discovery is anonymous and must not depend on an ambient
    // cookie or the durable authentication store.
    if request.uri().path() == "/auth/signup-options" {
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

#[cfg(test)]
mod tests {
    use axum::{body::Body, extract::Extension, http::Request, routing::get, Router};
    use layer2_core::Authorization;
    use tower::ServiceExt as _;

    use super::*;
    use crate::auth::AuthMode;

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
}
