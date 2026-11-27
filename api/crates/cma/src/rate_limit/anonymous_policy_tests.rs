use super::*;
use axum::extract::ConnectInfo;
use axum::{
    body::{to_bytes, Body},
    http::StatusCode,
    middleware,
    routing::any,
    Router,
};
use std::net::SocketAddr;
use std::num::NonZeroU32;
use tower::ServiceExt;

const PUBLIC: &[(&str, &str)] = &[
    ("GET", "/api/v1/triggers/public/pub_one"),
    ("GET", "/api/v1/triggers/public/pub_one/badge.svg"),
    ("GET", "/api/v1/triggers/public/pub_one/open/plc_one"),
    ("POST", "/api/v1/triggers/public/pub_one/entry-intent"),
];

fn local(enabled: bool, admission: u32, default: u32) -> Arc<RateLimiters> {
    let quota = |n| Quota::per_hour(NonZeroU32::new(n).unwrap());
    let mut limits = RateLimiters::from_quotas(
        quota(admission),
        quota(120),
        quota(default),
        quota(1),
        quota(120),
        quota(3),
        TrustedProxyConfig::sample(),
    );
    limits.control_tier_enabled = true;
    limits.anonymous_public = enabled.then(|| {
        AnonymousPublicBudget::new(NonZeroU32::new(1).unwrap(), NonZeroU32::new(1).unwrap())
    });
    Arc::new(limits)
}

fn request(method: &str, path: &str, peer: u8, cookie: bool) -> Request {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([192, 0, 2, peer], 443))));
    if cookie {
        request
            .headers_mut()
            .insert(header::COOKIE, "visitor=synthetic".parse().unwrap());
        request
            .extensions_mut()
            .insert(AuthenticatedSubject(layer2_core::SubjectId::new("visitor")));
    }
    request
}

fn router(limits: Arc<RateLimiters>) -> Router {
    let mut app = Router::new();
    for (_, path) in PUBLIC {
        app = app.route(path, any(|| async { StatusCode::NO_CONTENT }));
    }
    for path in [
        "/api/v1/triggers/public/{id}/launches",
        "/api/v1/agents/{id}/stop",
        "/api/v1/launches/{id}/cancel",
        "/api/v1/agents/{id}/approvals/{request_id}",
        "/auth/callback",
    ] {
        app = app.route(path, any(|| async { StatusCode::NO_CONTENT }));
    }
    app.layer(middleware::from_fn_with_state(limits.clone(), enforce))
        .layer(middleware::from_fn_with_state(limits, admit))
}

async fn limited(response: Response) {
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
    let retry = response.headers()[header::RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!(retry >= 1);
    assert!(response.headers().contains_key(header::LINK));
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["status"], 429);
}

#[tokio::test]
async fn every_public_route_shares_one_budget_across_networks_and_ambient_identity() {
    for cookie in [false, true] {
        for (method, path) in PUBLIC {
            let app = router(local(true, 100, 100));
            assert_eq!(
                app.clone()
                    .oneshot(request(method, path, 1, cookie))
                    .await
                    .unwrap()
                    .status(),
                204
            );
            for (index, (method, path)) in PUBLIC.iter().enumerate() {
                limited(
                    app.clone()
                        .oneshot(request(method, path, 2 + index as u8, cookie))
                        .await
                        .unwrap(),
                )
                .await;
            }
        }
    }
}

#[tokio::test]
async fn exhausted_anonymous_budget_preserves_preflight_launch_controls_and_oauth() {
    let limits = local(true, 100, 100);
    let app = router(limits.clone());
    assert_eq!(
        app.clone()
            .oneshot(request(PUBLIC[0].0, PUBLIC[0].1, 1, false))
            .await
            .unwrap()
            .status(),
        204
    );
    limited(
        app.clone()
            .oneshot(request(PUBLIC[0].0, PUBLIC[0].1, 2, true))
            .await
            .unwrap(),
    )
    .await;
    for (_, path) in PUBLIC {
        let mut preflight = request("OPTIONS", path, 1, false);
        preflight
            .headers_mut()
            .insert(header::ORIGIN, "https://visitor.example".parse().unwrap());
        preflight.headers_mut().insert(
            header::ACCESS_CONTROL_REQUEST_METHOD,
            "POST".parse().unwrap(),
        );
        assert_eq!(app.clone().oneshot(preflight).await.unwrap().status(), 204);
    }
    for (method, path) in [
        ("POST", "/api/v1/triggers/public/pub_one/launches"),
        ("POST", "/api/v1/agents/agt_one/stop"),
        ("POST", "/api/v1/launches/lch_one/cancel"),
        ("POST", "/api/v1/agents/agt_one/approvals/apr_one"),
        ("GET", "/auth/callback"),
    ] {
        assert_eq!(
            app.clone()
                .oneshot(request(method, path, 1, true))
                .await
                .unwrap()
                .status(),
            204,
            "{path}"
        );
    }
    limited(
        app.oneshot(request(
            "POST",
            "/api/v1/agents/agt_one/approvals/apr_one",
            1,
            true,
        ))
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(
        limits.rejections.load(Ordering::Relaxed),
        2,
        "Control retains its own finite budget"
    );
}

#[tokio::test]
async fn disabled_policy_preserves_per_client_admission_and_operation_budgets() {
    for (admission, default) in [(1, 100), (100, 1)] {
        let limits = local(false, admission, default);
        assert!(limits.anonymous_public.is_none());
        let app = router(limits);
        assert_eq!(
            app.clone()
                .oneshot(request("GET", PUBLIC[0].1, 1, true))
                .await
                .unwrap()
                .status(),
            204
        );
        limited(
            app.clone()
                .oneshot(request("GET", PUBLIC[0].1, 1, true))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            app.oneshot(request("GET", PUBLIC[0].1, 2, true))
                .await
                .unwrap()
                .status(),
            204
        );
    }
}

#[test]
fn malformed_or_unrelated_paths_are_outside_the_anonymous_policy() {
    for path in [
        "/api/v1/triggers/public/pub_one/",
        "/api/v1/triggers/public//pub_one",
        "/api/v1/triggers/public/pub_one/badge.svg/extra",
        "/api/v1/triggers/public/pub_one/entry-intent/extra",
        "/api/v1/triggers/public/pub_one/open",
        "/api/v1/triggers/public/pub_one/open/plc_one/extra",
        "/api/v2/triggers/public/pub_one",
        "/api/v1/triggers/public/pub_one/launches",
    ] {
        assert!(!is_anonymous_public_route(path), "{path}");
    }
}
