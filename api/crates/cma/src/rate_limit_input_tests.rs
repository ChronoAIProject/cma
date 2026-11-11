use super::*;
use axum::{
    body::{to_bytes, Body},
    http::StatusCode,
    middleware,
    routing::post,
    Router,
};
use layer2_core::SubjectId;
use std::num::NonZeroU32;
use tower::ServiceExt;

const INPUT_ROUTES: &[(&str, &str)] = &[
    ("/api/v2/responses", "/api/v2/responses"),
    (
        "/api/v2/agents/{agent_id}/responses",
        "/api/v2/agents/agt_one/responses",
    ),
    ("/api/v1/responses", "/api/v1/responses"),
    ("/api/v1/agui/run", "/api/v1/agui/run"),
    ("/api/v1/agents/{id}/input", "/api/v1/agents/agt_one/input"),
    ("/api/v1/agents/{id}/steer", "/api/v1/agents/agt_one/steer"),
    (
        "/api/v2/agents/{agent_id}/responses/{response_id}/queued-input",
        "/api/v2/agents/agt_one/responses/rsp_one/queued-input",
    ),
    (
        "/api/v2/agents/{agent_id}/responses/{response_id}/interrupt",
        "/api/v2/agents/agt_one/responses/rsp_one/interrupt",
    ),
];

fn request(method: Method, path: &str, subject: Option<&str>, binding: bool) -> Request {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    if let Some(subject) = subject {
        request
            .extensions_mut()
            .insert(AuthenticatedSubject(SubjectId::new(subject)));
    }
    if binding {
        request
            .extensions_mut()
            .insert(layer1_public::AuthMethod::BindingKey {
                binding_id: "binding-fixture".into(),
            });
    }
    request
}
fn local(enabled: bool) -> Arc<RateLimiters> {
    // A long refill interval makes exact burst boundaries deterministic.
    let quota = |n| Quota::per_hour(NonZeroU32::new(n).unwrap());
    let mut limiters = RateLimiters::from_quotas(
        quota(1000),
        quota(1000),
        quota(600),
        quota(10),
        quota(120),
        TrustedProxyConfig::sample(),
    );
    limiters.response_input_tier_enabled = enabled;
    Arc::new(limiters)
}
fn router(limiters: Arc<RateLimiters>) -> Router {
    let mut router = Router::new();
    for (path, _) in INPUT_ROUTES {
        router = router.route(path, post(|| async { StatusCode::NO_CONTENT }));
    }
    router
        .route(
            "/api/v1/conversations",
            post(|| async { StatusCode::CREATED }),
        )
        .route("/api/v1/sandboxes", post(|| async { StatusCode::CREATED }))
        .layer(middleware::from_fn_with_state(limiters, enforce))
}
async fn refusal(response: Response) {
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let delay = response.headers()["retry-after"]
        .to_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!(delay >= 1);
    let value: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
    assert_eq!(value["code"], "rate_limited");
}

#[tokio::test]
async fn exact_registered_input_routes_use_input_only_when_enabled() {
    for enabled in [false, true] {
        let mut app = Router::new();
        for (path, _) in INPUT_ROUTES {
            app = app.route(
                path,
                post(move |request: Request| async move { request_tier(&request, enabled).name() }),
            );
        }
        for (_, path) in INPUT_ROUTES {
            let response = app
                .clone()
                .oneshot(request(Method::POST, path, Some("owner"), false))
                .await
                .unwrap();
            let expected = if enabled || path.ends_with("/input") {
                "input"
            } else {
                "create"
            };
            assert_eq!(
                to_bytes(response.into_body(), 32).await.unwrap(),
                expected,
                "{path}, enabled={enabled}"
            );
            // Binding routing may allocate a source-user conversation. Route
            // identity alone cannot certify that this request only uses one.
            let response = app
                .clone()
                .oneshot(request(Method::POST, path, Some("owner"), true))
                .await
                .unwrap();
            let expected = if !enabled && path.ends_with("/input") {
                "input"
            } else {
                "create"
            };
            assert_eq!(to_bytes(response.into_body(), 32).await.unwrap(), expected);
        }
    }
}

#[test]
fn other_methods_creates_controls_and_near_matches_do_not_gain_input_budget() {
    for method in [
        Method::GET,
        Method::HEAD,
        Method::PUT,
        Method::PATCH,
        Method::DELETE,
        Method::OPTIONS,
    ] {
        for (_, path) in INPUT_ROUTES {
            assert_eq!(
                request_tier(&request(method.clone(), path, None, false), true),
                RequestTier::Default
            );
        }
    }
    for path in [
        "/api/v1/conversations",
        "/api/v2/conversations",
        "/api/v1/sandboxes",
        "/api/v1/workspaces/ws_one/agents",
        "/api/v1/triggers/public/pub_one/launches",
        "/api/v1/cmaeg/bindings/b/preparations",
        "/api/v2/responses/",
        "/api//v2/responses",
        "/API/v2/responses",
        "/api/v2/responses/extra",
        "/api/v3/responses",
        "/api/v2/agents//responses",
        "/unregistered/input",
        "/unregistered/responses",
        "/api/v2/agents/agt_one/responses/rsp_one/controls",
        "/api/v1/agents/agt_one/stop",
        "/api/v1/agents/agt_one/approvals/approval_one",
        "/api/v2/agents/agt_one/responses/rsp_one/queued-cancellation",
        "/api/v2/%72esponses",
        "/api/v2/agents/agt_one/responses%2fextra",
    ] {
        assert_eq!(
            request_tier(&request(Method::POST, path, None, false), true),
            RequestTier::Create,
            "{path}"
        );
    }
    // No matched route: a concrete dynamic URL is not mistaken for a template.
    assert_eq!(
        request_tier(
            &request(
                Method::POST,
                "/api/v2/agents/agt_one/responses",
                None,
                false
            ),
            true
        ),
        RequestTier::Create
    );
    assert_eq!(
        request_tier(
            &request(Method::POST, "/api/v2/responses?x=1", None, false),
            true
        ),
        RequestTier::Input
    );
}

#[tokio::test]
async fn aliases_share_the_configured_input_burst_and_leave_ten_creates_available() {
    let app = router(local(true));
    for n in 0..120 {
        let path = INPUT_ROUTES[n % INPUT_ROUTES.len()].1;
        assert_eq!(
            app.clone()
                .oneshot(request(Method::POST, path, Some("owner"), false))
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    refusal(
        app.clone()
            .oneshot(request(
                Method::POST,
                "/api/v2/responses",
                Some("owner"),
                false,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        app.clone()
            .oneshot(request(
                Method::POST,
                "/api/v2/responses",
                Some("another"),
                false
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    for _ in 0..10 {
        assert_eq!(
            app.clone()
                .oneshot(request(
                    Method::POST,
                    "/api/v1/conversations",
                    Some("owner"),
                    false
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
    }
    refusal(
        app.clone()
            .oneshot(request(
                Method::POST,
                "/api/v1/sandboxes",
                Some("owner"),
                false,
            ))
            .await
            .unwrap(),
    )
    .await;
}

#[tokio::test]
async fn disabled_response_submissions_still_share_create_budget_with_resource_creates() {
    let app = router(local(false));
    for _ in 0..10 {
        assert_eq!(
            app.clone()
                .oneshot(request(
                    Method::POST,
                    "/api/v2/responses",
                    Some("owner"),
                    false
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    for path in [
        "/api/v2/agents/agt_one/responses",
        "/api/v1/responses",
        "/api/v1/conversations",
    ] {
        refusal(
            app.clone()
                .oneshot(request(Method::POST, path, Some("owner"), false))
                .await
                .unwrap(),
        )
        .await;
    }
    assert_eq!(
        app.clone()
            .oneshot(request(
                Method::POST,
                "/api/v1/agents/agt_one/input",
                Some("owner"),
                false
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn binding_submissions_and_anonymous_networks_keep_isolated_finite_budgets() {
    let app = router(local(true));
    for _ in 0..10 {
        assert_eq!(
            app.clone()
                .oneshot(request(
                    Method::POST,
                    "/api/v2/responses",
                    Some("binding-owner"),
                    true
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    refusal(
        app.clone()
            .oneshot(request(
                Method::POST,
                "/api/v2/agents/agt_one/responses",
                Some("binding-owner"),
                true,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        app.clone()
            .oneshot(request(
                Method::POST,
                "/api/v2/responses",
                Some("binding-owner"),
                false
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    for _ in 0..120 {
        assert_eq!(
            app.clone()
                .oneshot(request(Method::POST, "/api/v2/responses", None, false))
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    refusal(
        app.clone()
            .oneshot(request(Method::POST, "/api/v2/responses", None, false))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        app.clone()
            .oneshot(request(
                Method::POST,
                "/api/v2/responses",
                Some("signed-in"),
                false
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
}

#[path = "rate_limit_input_mongo_tests.rs"]
mod mongo;
