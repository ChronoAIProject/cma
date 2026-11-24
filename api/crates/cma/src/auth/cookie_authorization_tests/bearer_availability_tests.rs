use super::*;
use axum::body::Body;
use axum::http::Request;
use layer1_public::AuthenticatedCaller;
use tower::ServiceExt as _;

fn application(fixture: &Fixture) -> Router {
    async fn probe(AuthenticatedCaller(caller): AuthenticatedCaller) -> StatusCode {
        if caller.authorization.allows(Capability::UseOwnSandboxes) {
            StatusCode::OK
        } else {
            StatusCode::FORBIDDEN
        }
    }
    Router::new()
        .route("/probe", get(probe))
        .layer(axum::middleware::from_fn_with_state(
            fixture.auth.clone(),
            crate::auth::attach_subject,
        ))
}

async fn request(app: &Router) -> Response {
    app.clone()
        .oneshot(
            Request::get("/probe")
                .header(header::AUTHORIZATION, "Bearer independent-fixture-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn bearer_provider_rate_limits_and_outages_return_503_then_recover_without_login() {
    for stage in ["discovery", "userinfo", "introspection"] {
        for status in [
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            let fixture = Fixture::new().await;
            let reply = match stage {
                "discovery" => &fixture.control.discovery,
                "userinfo" => &fixture.control.userinfo,
                _ => &fixture.control.introspection,
            };
            reply.write().await.status = status;
            let app = application(&fixture);
            let response = request(&app).await;
            assert_eq!(
                response.status(),
                StatusCode::SERVICE_UNAVAILABLE,
                "{stage}"
            );
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["code"], "identity_unavailable");
            assert_eq!(body["retryable"], true);
            assert!(!String::from_utf8_lossy(&bytes).contains("fixture-token"));
            reply.write().await.status = StatusCode::OK;
            assert_eq!(request(&app).await.status(), StatusCode::OK);
        }
    }
}

#[tokio::test]
async fn bearer_refusal_stays_unauthenticated_and_missing_permission_stays_forbidden() {
    let fixture = Fixture::new().await;
    let app = application(&fixture);
    for status in [StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN] {
        fixture.control.userinfo.write().await.status = status;
        assert_eq!(request(&app).await.status(), StatusCode::UNAUTHORIZED);
    }
    fixture.control.userinfo.write().await.status = StatusCode::OK;
    *fixture.control.introspection.write().await = Reply::json(json!({"active": false}));
    assert_eq!(request(&app).await.status(), StatusCode::UNAUTHORIZED);
    *fixture.control.introspection.write().await = Reply::json(json!({
        "active": true, "sub": "owner", "permissions": [],
    }));
    assert_eq!(request(&app).await.status(), StatusCode::FORBIDDEN);
    // Any explicit identity-boundary refusal rejects without retry or fallback.
    fixture.control.introspection.write().await.status = StatusCode::UNAUTHORIZED;
    assert_eq!(request(&app).await.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn bearer_transport_failure_and_timeout_return_unavailable() {
    let mut fixture = Fixture::new().await;
    Arc::get_mut(&mut fixture.auth).unwrap().set_fixture_http(
        reqwest::Client::builder()
            .timeout(Duration::from_millis(200))
            .build()
            .unwrap(),
    );
    fixture.control.userinfo.write().await.delay = Duration::from_secs(1);
    let app = application(&fixture);
    assert_eq!(
        request(&app).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    fixture.control.userinfo.write().await.delay = Duration::ZERO;
    assert_eq!(request(&app).await.status(), StatusCode::OK);
    fixture.server.abort();
    tokio::task::yield_now().await;
    assert_eq!(
        request(&app).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}
