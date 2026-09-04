use std::{collections::HashMap, sync::Arc};

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use tower::ServiceExt as _;

use super::*;
use crate::auth::{
    persistence::{AuthStore as _, MemoryAuthStore},
    ProviderMetadata,
};

#[tokio::test]
async fn registration_wraps_and_persists_the_complete_authorize_attempt() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = AuthState::with_store_and_nyxid_public_url(
        AuthMode::Provider {
            base_url: "http://provider.invalid".into(),
            client_id: "client-a".into(),
            client_secret: "secret".into(),
            public_url: "https://cma.example".into(),
        },
        None,
        store.clone(),
        Some("https://nyx.example".into()),
    );
    *auth.metadata.write().await = Some(ProviderMetadata {
        authorization_endpoint: "https://nyx-api.example/oauth/authorize".into(),
        token_endpoint: "https://nyx-api.example/oauth/token".into(),
        userinfo_endpoint: None,
        introspection_endpoint: None,
    });

    let response = begin_login(
        auth,
        Some("/t/pub_123/plc_456".into()),
        Some(LoginScreen::Register),
    )
    .await;

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let registration =
        reqwest::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    assert_eq!(
        registration.as_str().split('?').next(),
        Some("https://nyx.example/register")
    );
    let authorize = registration
        .query_pairs()
        .find_map(|(key, value)| (key == "return_to").then(|| value.into_owned()))
        .and_then(|value| reqwest::Url::parse(&value).ok())
        .expect("registration keeps the authorize URL");
    let query = authorize
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<HashMap<_, _>>();
    assert_eq!(query.get("client_id").map(String::as_str), Some("client-a"));
    assert_eq!(
        query.get("redirect_uri").map(String::as_str),
        Some("https://cma.example/auth/callback")
    );
    assert_eq!(
        query.get("scope").map(String::as_str),
        Some("openid profile email roles proxy offline_access")
    );

    let state = query.get("state").expect("authorize state");
    let binding = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with(BINDING_COOKIE_PREFIX))
        .and_then(|value| value.split(';').next())
        .and_then(|value| value.split_once('='))
        .map(|(_, value)| value)
        .expect("browser binding cookie");
    let attempt = store
        .take_login_attempt(
            &SecretDigest::identify(ATTEMPT_DOMAIN, state),
            &SecretDigest::identify(BINDING_DOMAIN, binding),
        )
        .await
        .unwrap()
        .expect("persisted login attempt");
    assert_eq!(attempt.return_to.as_deref(), Some("/t/pub_123/plc_456"));
    assert_eq!(
        query.get("code_challenge").map(String::as_str),
        Some(code_challenge(attempt.verifier.expose()).as_str())
    );
}

#[test]
fn return_targets_are_only_canonical_cma_workflows() {
    for valid in [
        "/",
        "/t/pub_123",
        "/t/pub_123/plc_456",
        "/triggers/manage",
        "/triggers/manage/trg_123",
        "/triggers/manage/trg_123/embed",
        "/agents/agt_123",
        "/sandboxes/sbx_123",
        "/profiles",
        "/profiles/agent/agp_123",
        "/profiles/workspace/wkp_123",
        "/profiles/sandbox/sbp_123",
        "/marketplace",
        "/marketplace/agent/agp_123",
    ] {
        assert_eq!(app_return_target(valid).as_deref(), Ok(valid));
    }
    for invalid in [
        "",
        "//example.com/t/pub_123",
        "https://example.com/t/pub_123",
        "/t",
        "/t/",
        "/t/pub_123/placement/extra",
        "/t/pub_123?next=https://example.com",
        "/t/pub_123%2Fextra",
        "/triggers",
        "/agents/..",
        "/profiles/unknown/prf_123",
        "/profiles/agent/prf_123/edit",
        "/sessions/old",
        "/launches/old",
    ] {
        assert!(app_return_target(invalid).is_err(), "accepted {invalid}");
    }
}

#[tokio::test]
async fn login_rejects_unknown_or_duplicate_screens() {
    for query in ["screen=", "screen=login", "screen=register&screen=register"] {
        let app = router(AuthState::in_memory(
            AuthMode::Trusted {
                subject: "developer".into(),
            },
            None,
        ));
        let response = app
            .oneshot(
                Request::get(format!("/auth/login?{query}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{query}");
    }
}

#[tokio::test]
async fn trusted_login_restores_a_valid_relative_target() {
    let auth = AuthState::in_memory(
        AuthMode::Trusted {
            subject: "developer".into(),
        },
        None,
    );
    let response = login(
        State(auth),
        Query(LoginParams {
            return_to: Some("/marketplace".into()),
            screen: None,
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/marketplace");
}
