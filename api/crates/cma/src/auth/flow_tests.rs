use axum::{body::Body, http::Request};
use tower::ServiceExt as _;

use super::*;

#[tokio::test]
async fn trusted_mode_keeps_the_browser_auth_routes_available() {
    let auth = AuthState::in_memory(
        AuthMode::Trusted {
            subject: "developer".into(),
        },
        None,
    );
    let app = router(auth.clone()).layer(axum::middleware::from_fn_with_state(
        auth,
        crate::auth::attach_subject,
    ));
    let login = app
        .clone()
        .oneshot(Request::get("/auth/login").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::SEE_OTHER);
    let current = app
        .oneshot(Request::get("/auth/session").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(current.status(), StatusCode::OK);
}

#[test]
fn pkce_challenges_and_query_values_are_url_safe() {
    let challenge = code_challenge("verifier");
    assert!(!challenge.contains('='));
    assert_eq!(urlencode("a/b"), "a%2Fb");
}

mod silent {
    use std::{collections::HashMap, sync::Arc};

    use super::*;
    use crate::auth::{
        persistence::{AuthStore as _, MemoryAuthStore},
        ProviderMetadata,
    };

    async fn provider(store: Arc<MemoryAuthStore>) -> Arc<AuthState> {
        let auth = AuthState::with_store_and_nyxid_public_url(
            AuthMode::Provider {
                base_url: "http://provider.invalid".into(),
                client_id: "client-a".into(),
                client_secret: "secret".into(),
                public_url: "https://cma.example".into(),
            },
            None,
            store,
            Some("https://nyx.example".into()),
        );
        *auth.metadata.write().await = Some(ProviderMetadata {
            authorization_endpoint: "https://nyx-api.example/oauth/authorize".into(),
            token_endpoint: "https://nyx-api.example/oauth/token".into(),
            userinfo_endpoint: None,
            introspection_endpoint: None,
        });
        auth
    }

    fn get(path: &str, cookie: Option<&str>) -> Request<Body> {
        let mut request = Request::get(path).header(header::HOST, "cma.example");
        if let Some(cookie) = cookie {
            request = request.header(header::COOKIE, cookie);
        }
        request.body(Body::empty()).unwrap()
    }

    fn binding_cookie(response: &Response) -> String {
        response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .find(|value| value.starts_with(BINDING_COOKIE_PREFIX))
            .and_then(|value| value.split(';').next())
            .expect("browser binding cookie")
            .to_owned()
    }

    fn authorize_query(response: &Response) -> HashMap<String, String> {
        reqwest::Url::parse(response.headers()[header::LOCATION].to_str().unwrap())
            .unwrap()
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect()
    }

    #[tokio::test]
    async fn silent_login_asks_the_provider_not_to_interact() {
        let store = Arc::new(MemoryAuthStore::default());
        let app = router(provider(store.clone()).await);
        let response = app
            .oneshot(get(
                "/auth/login?prompt=none&return_to=%2Fagents%2Fagt_123",
                None,
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let query = authorize_query(&response);
        assert_eq!(query.get("prompt").map(String::as_str), Some("none"));
        let cookie = binding_cookie(&response);
        let (_, binding) = cookie.split_once('=').unwrap();
        let attempt = store
            .take_login_attempt(
                &SecretDigest::identify(ATTEMPT_DOMAIN, &query["state"]),
                &SecretDigest::identify(BINDING_DOMAIN, binding),
            )
            .await
            .unwrap()
            .expect("persisted login attempt");
        assert!(attempt.silent);
        assert_eq!(attempt.return_to.as_deref(), Some("/agents/agt_123"));
    }

    #[tokio::test]
    async fn interactive_login_does_not_send_a_prompt() {
        let app = router(provider(Arc::new(MemoryAuthStore::default())).await);
        let response = app.oneshot(get("/auth/login", None)).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert!(!authorize_query(&response).contains_key("prompt"));
    }

    #[tokio::test]
    async fn a_declined_silent_login_returns_to_the_page_it_left() {
        let app = router(provider(Arc::new(MemoryAuthStore::default())).await);
        let login = app
            .clone()
            .oneshot(get(
                "/auth/login?prompt=none&return_to=%2Fagents%2Fagt_123",
                None,
            ))
            .await
            .unwrap();
        let state = authorize_query(&login)["state"].clone();
        let cookie = binding_cookie(&login);

        let callback = app
            .oneshot(get(
                &format!("/auth/callback?state={state}&error=login_required"),
                Some(&cookie),
            ))
            .await
            .unwrap();

        assert_eq!(callback.status(), StatusCode::SEE_OTHER);
        assert_eq!(callback.headers()[header::LOCATION], "/agents/agt_123");
        let cookies = callback
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let (name, _) = cookie.split_once('=').unwrap();
        assert!(cookies
            .iter()
            .any(|value| value.starts_with(&format!("{name}=;")) && value.contains("Max-Age=0")));
        assert!(!cookies.iter().any(|value| value.starts_with(AUTH_COOKIE)));
    }

    #[tokio::test]
    async fn a_declined_interactive_login_is_still_reported() {
        let app = router(provider(Arc::new(MemoryAuthStore::default())).await);
        let login = app.clone().oneshot(get("/auth/login", None)).await.unwrap();
        let state = authorize_query(&login)["state"].clone();
        let cookie = binding_cookie(&login);

        let callback = app
            .oneshot(get(
                &format!("/auth/callback?state={state}&error=access_denied"),
                Some(&cookie),
            ))
            .await
            .unwrap();

        assert_eq!(callback.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn silent_login_refuses_other_prompts_and_provider_screens() {
        for query in [
            "prompt=login",
            "prompt=consent",
            "prompt=",
            "prompt=none&screen=register",
        ] {
            let app = router(provider(Arc::new(MemoryAuthStore::default())).await);
            let response = app
                .oneshot(get(&format!("/auth/login?{query}"), None))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{query}");
        }
    }
}
