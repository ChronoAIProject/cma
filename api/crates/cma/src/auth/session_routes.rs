//! What a signed-in browser can ask about itself, and how it signs out.
//!
//! Split from the flow so that file is about getting somebody signed in and
//! this one about the two things they do afterwards. Both are short, and both
//! are read by whoever is writing a frontend rather than by whoever is
//! debugging the dance.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};

use super::{session_cookies, AuthState, LEGACY_SESSION_COOKIE, SESSION_COOKIE};

pub(super) async fn session(State(auth): State<Arc<AuthState>>, headers: HeaderMap) -> Response {
    match auth.resolve_request(&headers).await {
        Ok(Some(resolved)) => {
            // The identifier stays, because clients key on it; the display
            // details ride alongside so nothing has to render a UUID at a
            // person.
            let subject = resolved.subject;
            let profile = resolved.profile;
            // What this caller may do, so a client can offer only what will be
            // permitted rather than guessing from a role name it has no
            // business knowing. Advisory: every one of these is checked again
            // by the core on the request that uses it.
            let entitled = resolved.authorization;
            let mut body = serde_json::json!({
                "authenticated": true,
                "subject": subject.as_str(),
                "capabilities": super::entitlements::reported(&entitled),
            });
            if let Some(map) = body.as_object_mut() {
                if let Ok(serde_json::Value::Object(fields)) = serde_json::to_value(profile) {
                    map.extend(fields);
                }
            }
            Json(body).into_response()
        }
        Ok(None) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "authenticated": false })),
        )
            .into_response(),
        Err(error) => {
            tracing::error!(error = %error, "could not resolve the browser session");
            layer1_public::auth::StoreUnavailable.into_response()
        }
    }
}

pub(super) async fn logout(State(auth): State<Arc<AuthState>>, headers: HeaderMap) -> Response {
    for token in session_cookies(&headers) {
        if let Err(error) = auth.revoke_session(&token).await {
            tracing::error!(error = %error, "could not revoke a browser session");
            return layer1_public::auth::StoreUnavailable.into_response();
        }
    }

    let mut response = StatusCode::SEE_OTHER.into_response();
    let secure = if auth.secure_cookies() {
        "; Secure"
    } else {
        ""
    };
    for name in [SESSION_COOKIE, LEGACY_SESSION_COOKIE] {
        response.headers_mut().append(
            header::SET_COOKIE,
            format!("{name}=; Path=/; HttpOnly{secure}; SameSite=Lax; Max-Age=0")
                .parse()
                .expect("a cookie name forms a header"),
        );
    }
    response
        .headers_mut()
        .insert(header::LOCATION, HeaderValue::from_static("/"));
    response
}

pub(super) fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::persistence::MemoryAuthStore;
    use crate::auth::AuthMode;

    fn provider_auth() -> Arc<AuthState> {
        AuthState::new(
            AuthMode::Provider {
                base_url: "http://provider.invalid".into(),
                client_id: "client".into(),
                client_secret: "not-a-real-value".into(),
                public_url: "http://service.invalid".into(),
            },
            None,
        )
    }

    #[tokio::test]
    async fn logout_revokes_and_expires_both_cookie_generations() {
        let auth = provider_auth();
        let canonical = auth
            .issue_session(layer2_core::SubjectId("canonical-subject".into()))
            .await
            .expect("canonical session persists");
        let legacy = auth
            .issue_session(layer2_core::SubjectId("legacy-subject".into()))
            .await
            .expect("legacy session persists");
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("{LEGACY_SESSION_COOKIE}={legacy}; {SESSION_COOKIE}={canonical}")
                .parse()
                .expect("valid cookie header"),
        );

        let response = logout(State(Arc::clone(&auth)), headers.clone()).await;

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers().get(header::LOCATION).unwrap(), "/");
        let expired = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().expect("visible cookie header"))
            .collect::<Vec<_>>();
        assert_eq!(
            expired,
            [
                "cma_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0",
                "chrono_bot_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0",
            ]
        );
        assert_eq!(
            auth.resolve(&headers)
                .await
                .expect("store remains available"),
            None
        );
    }

    #[tokio::test]
    async fn logout_store_failure_is_a_typed_503_and_does_not_fake_success() {
        let store = Arc::new(MemoryAuthStore::default());
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: "http://provider.invalid".into(),
                client_id: "client".into(),
                client_secret: "not-a-real-value".into(),
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
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("{SESSION_COOKIE}={cookie}")
                .parse()
                .expect("cookie header"),
        );

        let response = logout(State(auth), headers).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("problem body"),
        )
        .expect("problem json");
        assert_eq!(body["code"], "store_failed");
    }

    #[tokio::test]
    async fn an_https_logout_clears_only_secure_cookie_variants() {
        let auth = AuthState::new(
            AuthMode::Provider {
                base_url: "http://provider.invalid".into(),
                client_id: "client".into(),
                client_secret: "not-a-real-value".into(),
                public_url: "https://cma.example".into(),
            },
            None,
        );
        let response = logout(State(auth), HeaderMap::new()).await;
        let expired = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().expect("visible cookie header"))
            .collect::<Vec<_>>();
        assert_eq!(
            expired,
            [
                "cma_session=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0",
                "chrono_bot_session=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0",
            ]
        );
    }
}
