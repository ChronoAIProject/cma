//! Exact deployment-owned browser origins during a canonical domain migration.
use super::{AuthMode, AuthState};
use axum::http::{header, HeaderMap, HeaderValue, Uri};
use axum::response::{IntoResponse, Redirect, Response};
use std::sync::Arc;

impl AuthState {
    pub fn approval_review_origin(&self) -> &str {
        &self.nyxid_public_url
    }

    pub fn with_browser_origins(mut self: Arc<Self>, origins: &[HeaderValue]) -> Arc<Self> {
        let state = Arc::get_mut(&mut self).expect("configure before sharing");
        for origin in origins {
            let origin = origin
                .to_str()
                .expect("validated browser origin")
                .to_owned();
            if !state.browser_origins.contains(&origin) {
                state.browser_origins.push(origin);
            }
        }
        self
    }

    pub(super) fn permits_browser_origin(&self, headers: &HeaderMap) -> bool {
        let mut values = headers.get_all(header::ORIGIN).iter();
        values
            .next()
            .and_then(|v| v.to_str().ok())
            .is_some_and(|origin| self.browser_origins.iter().any(|v| v == origin))
            && values.next().is_none()
    }

    /// Move login entry navigation before issuing a host-only PKCE cookie.
    /// Callbacks and credential-bearing API requests never use this redirect.
    pub(super) fn canonical_login_redirect(
        &self,
        headers: &HeaderMap,
        uri: &Uri,
    ) -> Option<Response> {
        let AuthMode::Provider { public_url, .. } = &self.mode else {
            return None;
        };
        let canonical = reqwest::Url::parse(public_url).ok()?;
        let incoming = headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .or_else(|| uri.authority().map(|value| value.as_str()))?;
        if incoming == &canonical[url::Position::BeforeHost..url::Position::AfterPort] {
            return None;
        }
        let path = uri.path_and_query()?.as_str();
        Some(
            Redirect::temporary(&format!("{}{path}", public_url.trim_end_matches('/')))
                .into_response(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn auth() -> Arc<AuthState> {
        AuthState::in_memory(
            AuthMode::Provider {
                base_url: "https://identity.example".into(),
                client_id: "client".into(),
                client_secret: "fixture-client-secret".into(),
                public_url: "https://new.example".into(),
            },
            None,
        )
        .with_browser_origins(&[HeaderValue::from_static("https://old.example")])
    }
    #[test]
    fn browser_aliases_are_exact_and_never_allow_duplicate_origins() {
        let auth = auth();
        let mut headers = HeaderMap::new();
        assert!(!auth.permits_browser_origin(&headers));
        for origin in [
            "null",
            "https://foreign.example",
            "https://old.example.evil",
            "https://old.example/",
        ] {
            headers.insert(header::ORIGIN, origin.parse().unwrap());
            assert!(!auth.permits_browser_origin(&headers));
        }
        for origin in ["https://new.example", "https://old.example"] {
            headers.insert(header::ORIGIN, origin.parse().unwrap());
            assert!(auth.permits_browser_origin(&headers));
        }
        headers.append(
            header::ORIGIN,
            HeaderValue::from_static("https://new.example"),
        );
        assert!(!auth.permits_browser_origin(&headers));
    }
    #[test]
    fn canonical_login_navigation_preserves_return_path_without_issuing_a_cookie() {
        let auth = auth();
        let mut headers = HeaderMap::new();
        let uri: Uri = "/auth/login?return_to=%2Fagents%2Fexample".parse().unwrap();
        headers.insert(header::HOST, HeaderValue::from_static("old.example"));
        let response = auth.canonical_login_redirect(&headers, &uri).unwrap();
        assert_eq!(
            response.status(),
            axum::http::StatusCode::TEMPORARY_REDIRECT
        );
        assert_eq!(
            response.headers()[header::LOCATION],
            "https://new.example/auth/login?return_to=%2Fagents%2Fexample"
        );
        assert!(!response.headers().contains_key(header::SET_COOKIE));
        headers.insert(header::HOST, HeaderValue::from_static("new.example"));
        assert!(auth.canonical_login_redirect(&headers, &uri).is_none());
    }
}
