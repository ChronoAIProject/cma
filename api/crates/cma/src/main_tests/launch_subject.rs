use super::*;

#[tokio::test]
async fn trigger_route_families_are_composed_before_the_rest_fallback() {
    let app = composed_rest_router();
    for (method, path) in [
        (Method::POST, "/api/v1/triggers"),
        (Method::GET, "/api/v1/triggers/trg_missing/embed"),
        (Method::POST, "/api/v1/triggers/public/pub_missing/launches"),
        (Method::POST, "/api/v2/triggers/public/pub_missing/launches"),
        (Method::GET, "/api/v1/launches"),
        (Method::GET, "/api/v1/service-connections/hnd_missing"),
        (Method::GET, "/api/v1/agui/launches/lnch_missing/events"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(response.headers().get(header::LINK), Some(&REST_LINKS));
    }
}

#[tokio::test]
async fn bound_command_checks_the_identity_resolved_from_the_actual_browser_cookie() {
    use crate::auth::persistence::{AuthStore, BrowserLogin, MemoryAuthStore, SecretDigest};
    use crate::auth::{attach_subject, AuthMode, AuthState};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use std::time::{Duration, SystemTime};
    let store = Arc::new(MemoryAuthStore::default());
    for (cookie, owner) in [("fixture-a", "owner-a"), ("fixture-b", "owner-b")] {
        store
            .put_browser_login(BrowserLogin {
                id: SecretDigest::identify("browser-login", cookie),
                subject: SubjectId::new(owner),
                expires_at: SystemTime::now() + Duration::from_secs(600),
            })
            .await
            .unwrap();
    }
    let auth = AuthState::with_store(
        AuthMode::Provider {
            base_url: "http://127.0.0.1:1".into(),
            client_id: "fixture".into(),
            client_secret: "fixture-value".into(),
            public_url: "https://example.test".into(),
        },
        None,
        store,
    );
    let app =
        composed_rest_router().layer(axum::middleware::from_fn_with_state(auth, attach_subject));
    let expected = format!("v1.{}", URL_SAFE_NO_PAD.encode("owner-a"));
    for (cookie, status, code, verified) in [
        (
            Some("fixture-b"),
            StatusCode::CONFLICT,
            "command_subject_mismatch",
            false,
        ),
        (Some("fixture-a"), StatusCode::FORBIDDEN, "forbidden", true),
        (None, StatusCode::UNAUTHORIZED, "not_authenticated", false),
    ] {
        let mut request = Request::post("/api/v2/triggers/public/pub_missing/launches")
            .header("content-type", "application/json")
            .header("idempotency-key", "original-owner-command")
            .header("x-cma-expected-subject", &expected);
        if let Some(cookie) = cookie {
            request = request.header("cookie", format!("cma_session={cookie}"));
        }
        let response = app
            .clone()
            .oneshot(
                request
                    .body(Body::from("{\"start_if_ready\":false}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(
            response
                .headers()
                .get("x-cma-verified-subject")
                .map(|v| v.to_str().unwrap()),
            verified.then_some(expected.as_str())
        );
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let problem: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(problem["code"], code);
    }
}
