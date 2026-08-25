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

use super::{AuthState, LEGACY_SESSION_COOKIE, SESSION_COOKIE};

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
    let resolved = match auth.resolve_request(req.headers()).await {
        Ok(resolved) => resolved,
        Err(error) => {
            tracing::error!(error = %error, "could not resolve identity from the auth store");
            return StoreUnavailable.into_response();
        }
    };
    if let Some(resolved) = resolved {
        // The credential rides separately from identity: nearly every handler
        // needs the subject, almost none needs a credential, so taking one has
        // to be deliberate.
        if let Some(token) = resolved.delegated_credential {
            let credential = if resolved.credential_is_request_bound {
                layer1_public::auth::DelegatedCredential::request_bound(token)
            } else {
                layer1_public::auth::DelegatedCredential::subject(token)
            };
            req.extensions_mut().insert(credential);
        }
        req.extensions_mut().insert(resolved.authorization);
        req.extensions_mut().insert(resolved.profile);
        req.extensions_mut()
            .insert(AuthenticatedSubject(resolved.subject));
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::{to_bytes, Body},
        extract::Extension,
        http::{header, Request, StatusCode},
        routing::get,
        Json, Router,
    };
    use layer1_public::auth::DelegatedCredential;
    use layer2_core::{Authorization, Capability, SubjectId};
    use tower::ServiceExt;

    use super::*;
    use crate::auth::persistence::{AuthStore, DurableSubjectTokens, MemoryAuthStore, SecretValue};
    use crate::auth::profile::Profile;
    use crate::auth::AuthMode;

    async fn inspect_request(
        Extension(subject): Extension<AuthenticatedSubject>,
        Extension(authorization): Extension<Authorization>,
        Extension(credential): Extension<DelegatedCredential>,
        Extension(profile): Extension<Profile>,
    ) -> Json<serde_json::Value> {
        Json(serde_json::json!({
            "subject": subject.0.as_str(),
            "create": authorization.allows(Capability::CreateSession),
            "read_all": authorization.allows(Capability::ReadAllSessions),
            "credential": credential.value(),
            "request_bound": credential.is_request_bound(),
            "display_name": profile.display_name,
        }))
    }

    fn bearer_request(token: &str) -> Request<Body> {
        Request::get("/inspect")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .expect("request builds")
    }

    async fn response_json(response: Response) -> serde_json::Value {
        assert_eq!(response.status(), StatusCode::OK);
        serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("response body"),
        )
        .expect("response json")
    }

    async fn same_subject_bearer_fixture() -> (Arc<AuthState>, Arc<MemoryAuthStore>, Router) {
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
        let subject = SubjectId("shared-subject".into());
        auth.store_sign_in(
            &subject,
            DurableSubjectTokens {
                access_token: SecretValue::new("browser-access".into()),
                refresh_token: Some(SecretValue::new("browser-refresh".into())),
                expires_at: std::time::SystemTime::now() + std::time::Duration::from_secs(3600),
            },
            Authorization::from_capabilities([Capability::UseOwnSessions]),
            Profile {
                display_name: Some("Browser profile".into()),
                email: None,
                avatar_url: None,
            },
        )
        .await
        .expect("browser subject state persists");
        auth.bearer
            .remember(
                "token-a",
                subject.clone(),
                Authorization::from_capabilities([Capability::CreateSession]),
                Profile {
                    display_name: Some("Token A".into()),
                    email: None,
                    avatar_url: None,
                },
            )
            .await;
        auth.bearer
            .remember(
                "token-b",
                subject,
                Authorization::from_capabilities([Capability::ReadAllSessions]),
                Profile {
                    display_name: Some("Token B".into()),
                    email: None,
                    avatar_url: None,
                },
            )
            .await;
        let app = Router::new().route("/inspect", get(inspect_request)).layer(
            axum::middleware::from_fn_with_state(Arc::clone(&auth), attach_subject),
        );
        (auth, store, app)
    }

    fn assert_token_a(value: &serde_json::Value) {
        assert_eq!(value["subject"], "shared-subject");
        assert_eq!(value["create"], true);
        assert_eq!(value["read_all"], false);
        assert_eq!(value["credential"], "token-a");
        assert_eq!(value["request_bound"], true);
        assert_eq!(value["display_name"], "Token A");
    }

    fn assert_token_b(value: &serde_json::Value) {
        assert_eq!(value["subject"], "shared-subject");
        assert_eq!(value["create"], false);
        assert_eq!(value["read_all"], true);
        assert_eq!(value["credential"], "token-b");
        assert_eq!(value["request_bound"], true);
        assert_eq!(value["display_name"], "Token B");
    }

    #[tokio::test]
    async fn same_subject_bearers_remain_deterministically_request_bound() {
        let (_auth, store, app) = same_subject_bearer_fixture().await;
        let first = response_json(
            app.clone()
                .oneshot(bearer_request("token-a"))
                .await
                .unwrap(),
        )
        .await;
        let second = response_json(
            app.clone()
                .oneshot(bearer_request("token-b"))
                .await
                .unwrap(),
        )
        .await;
        let repeated = response_json(app.oneshot(bearer_request("token-a")).await.unwrap()).await;
        assert_token_a(&first);
        assert_token_b(&second);
        assert_token_a(&repeated);

        let durable = store
            .subject(&SubjectId("shared-subject".into()))
            .await
            .expect("store works")
            .expect("browser state remains");
        assert_eq!(
            durable.profile.display_name.as_deref(),
            Some("Browser profile")
        );
        assert_eq!(
            durable
                .tokens
                .expect("renewable browser tokens remain")
                .access_token
                .expose(),
            "browser-access"
        );
    }

    #[tokio::test]
    async fn concurrent_same_subject_bearers_cannot_exchange_request_authority() {
        let (_auth, _store, app) = same_subject_bearer_fixture().await;
        let (left, right) = tokio::join!(
            app.clone().oneshot(bearer_request("token-a")),
            app.oneshot(bearer_request("token-b")),
        );
        let (left, right) = tokio::join!(
            response_json(left.expect("token A response")),
            response_json(right.expect("token B response")),
        );
        assert_token_a(&left);
        assert_token_b(&right);
    }

    #[tokio::test]
    async fn browser_credentials_are_marked_as_subject_renewable() {
        let (auth, _store, app) = same_subject_bearer_fixture().await;
        let cookie = auth
            .issue_session(SubjectId("shared-subject".into()))
            .await
            .expect("browser session persists");
        let value = response_json(
            app.oneshot(
                Request::get("/inspect")
                    .header(
                        header::COOKIE,
                        format!("{}={cookie}", super::SESSION_COOKIE),
                    )
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers"),
        )
        .await;
        assert_eq!(value["credential"], "browser-access");
        assert_eq!(value["request_bound"], false);
    }

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
