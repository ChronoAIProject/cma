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
