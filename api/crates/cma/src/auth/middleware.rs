//! Putting the authenticated subject on the request.
//!
//! Split from the state so that file is about identity and this one about the
//! single place identity meets a request. Everything downstream reads what this
//! attaches and never a header, which is what stops an interface from acquiring
//! a user model of its own.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{header, HeaderMap},
    response::{IntoResponse, Response},
};

use layer1_public::auth::StoreUnavailable;
use layer1_public::AuthenticatedSubject;

use super::{AuthMode, AuthState, LEGACY_SESSION_COOKIE, SESSION_COOKIE};

pub(crate) fn session_cookies(headers: &HeaderMap) -> Vec<String> {
    let Some(raw) = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
    else {
        return Vec::new();
    };

    [SESSION_COOKIE, LEGACY_SESSION_COOKIE]
        .into_iter()
        .filter_map(|name| {
            raw.split(';')
                .filter_map(|part| part.trim().split_once('='))
                .find(|(candidate, _)| *candidate == name)
                .map(|(_, value)| value.to_string())
        })
        .collect()
}

/// Middleware attaching the authenticated subject to the request.
pub async fn attach_subject(
    State(auth): State<Arc<AuthState>>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let subject = match auth.resolve(req.headers()).await {
        Ok(subject) => subject,
        Err(error) => {
            tracing::error!(error = %error, "could not resolve identity from the auth store");
            return StoreUnavailable.into_response();
        }
    };
    if let Some(subject) = subject {
        // The credential rides separately from identity: nearly every handler
        // needs the subject, almost none needs a credential, so taking one has
        // to be deliberate.
        match &auth.mode {
            AuthMode::Trusted { .. } => {
                req.extensions_mut()
                    .insert(AuthState::trusted_authorization());
            }
            AuthMode::Provider { .. } => {
                let fact = match auth.subject_fact(&subject).await {
                    Ok(fact) => fact,
                    Err(error) => {
                        tracing::error!(
                            %subject,
                            error = %error,
                            "could not read subject authentication state"
                        );
                        return StoreUnavailable.into_response();
                    }
                };
                if let Some(fact) = fact {
                    if let Some(tokens) = fact
                        .tokens
                        .filter(|tokens| tokens.expires_at > std::time::SystemTime::now())
                    {
                        req.extensions_mut()
                            .insert(layer1_public::auth::DelegatedCredential(
                                tokens.access_token.into_inner(),
                            ));
                    }
                    req.extensions_mut().insert(fact.authorization);
                    req.extensions_mut().insert(fact.profile);
                } else {
                    req.extensions_mut()
                        .insert(layer2_core::Authorization::none());
                }
            }
        }
        req.extensions_mut().insert(AuthenticatedSubject(subject));
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::{to_bytes, Body},
        http::{header, Request, StatusCode},
        routing::get,
        Router,
    };
    use tower::ServiceExt;

    use super::*;
    use crate::auth::persistence::MemoryAuthStore;

    #[tokio::test]
    async fn cookie_store_failure_is_a_typed_503_not_a_401() {
        let store = Arc::new(MemoryAuthStore::default());
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: "http://provider.invalid".into(),
                client_id: "client".into(),
                client_secret: "secret".into(),
                public_url: "http://service.invalid".into(),
            },
            None,
            store.clone(),
        );
        let cookie = auth
            .issue_session(layer2_core::SubjectId("subject-a".into()))
            .await
            .expect("session persists");
        store.set_unavailable(true);
        let app = Router::new()
            .route("/protected", get(|| async { StatusCode::NO_CONTENT }))
            .layer(axum::middleware::from_fn_with_state(
                Arc::clone(&auth),
                attach_subject,
            ));

        let response = app
            .oneshot(
                Request::get("/protected")
                    .header(
                        header::COOKIE,
                        format!("{}={cookie}", super::SESSION_COOKIE),
                    )
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("problem body"),
        )
        .expect("problem json");
        assert_eq!(body["code"], "store_failed");
    }
}
