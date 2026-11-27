use super::*;
use axum::{
    body::{to_bytes, Body},
    http::StatusCode,
    middleware,
    routing::{any, get, post},
    Router,
};
use layer2_core::SubjectId;
use std::{num::NonZeroU32, time::Instant};
use tower::ServiceExt;

// Verified against rest/agent_operate.rs, rest/agent_access_requests.rs,
// rest/launches.rs, responses/{mod,v2/mod}.rs, rest/sandboxes.rs,
// rest/sandbox_commands.rs and rest/agent_capacity_recovery/commands.rs.
const CONTROL_ROUTES: &[(&str, &str)] = &[
    ("/api/v1/agents/{id}/stop", "/api/v1/agents/agt_one/stop"),
    (
        "/api/v1/agents/{id}/interrupt",
        "/api/v1/agents/agt_one/interrupt",
    ),
    (
        "/api/v1/agents/{id}/approvals/{request_id}",
        "/api/v1/agents/agt_one/approvals/apr_one",
    ),
    (
        "/api/v1/agents/{id}/access-requests/{request_id}/approve",
        "/api/v1/agents/agt_one/access-requests/req_one/approve",
    ),
    (
        "/api/v1/agents/{id}/access-requests/{request_id}/deny",
        "/api/v1/agents/agt_one/access-requests/req_one/deny",
    ),
    (
        "/api/v1/launches/{id}/cancel",
        "/api/v1/launches/lch_one/cancel",
    ),
    (
        "/api/v1/responses/{response_id}/cancel",
        "/api/v1/responses/rsp_one/cancel",
    ),
    (
        "/api/v1/sandboxes/{id}/pause",
        "/api/v1/sandboxes/sbx_one/pause",
    ),
    (
        "/api/v1/sandboxes/{id}/commands",
        "/api/v1/sandboxes/sbx_one/commands",
    ),
    (
        "/api/v2/agents/{agent_id}/responses/{response_id}/interrupt",
        "/api/v2/agents/agt_one/responses/rsp_one/interrupt",
    ),
    (
        "/api/v2/agents/{agent_id}/responses/{response_id}/controls",
        "/api/v2/agents/agt_one/responses/rsp_one/controls",
    ),
    (
        "/api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation",
        "/api/v2/agents/agt_one/responses/rsp_one/queued-cancellation",
    ),
    (
        "/api/v1/sandboxes/{id}/revive",
        "/api/v1/sandboxes/sbx_one/revive",
    ),
    (
        "/api/v1/agents/{id}/capacity-recovery/resume",
        "/api/v1/agents/agt_one/capacity-recovery/resume",
    ),
];

fn request(method: Method, path: &str, owner: &str) -> Request {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(AuthenticatedSubject(SubjectId::new(owner)));
    request
}

fn local(enabled: bool) -> Arc<RateLimiters> {
    // Long refill intervals keep burst exhaustion deterministic on busy hosts.
    let quota = |n| Quota::per_hour(NonZeroU32::new(n).unwrap());
    let mut limiters = RateLimiters::from_quotas(
        quota(1000),
        quota(120),
        quota(600),
        quota(10),
        quota(120),
        quota(120),
        TrustedProxyConfig::sample(),
    );
    limiters.response_input_tier_enabled = enabled;
    limiters.control_tier_enabled = true;
    Arc::new(limiters)
}

fn router(limiters: Arc<RateLimiters>) -> Router {
    let mut app = Router::new();
    for (template, _) in CONTROL_ROUTES {
        app = app.route(template, post(|| async { StatusCode::NO_CONTENT }));
    }
    app.route("/api/v1/sandboxes", post(|| async { StatusCode::CREATED }))
        .route(
            "/api/v1/agents/{id}/input",
            post(|| async { StatusCode::NO_CONTENT }),
        )
        .route(
            "/api/v2/responses",
            post(|| async { StatusCode::NO_CONTENT }),
        )
        .route("/api/v1/agents", get(|| async { StatusCode::OK }))
        .layer(middleware::from_fn_with_state(limiters, enforce))
}

async fn refusal(response: Response) {
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
    let retry_after = response.headers()[header::RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!(retry_after >= 1);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
    assert_eq!(body["code"], "rate_limited");
    assert_eq!(body["status"], 429);
}

#[tokio::test]
async fn exact_registered_recovery_and_controls_precede_input_policy_for_all_auth_methods() {
    for enabled in [false, true] {
        let mut app = Router::new();
        for (template, _) in CONTROL_ROUTES {
            app = app.route(
                template,
                any(move |request: Request| async move {
                    let tier = request_tier(&request, enabled, true).name();
                    ([("x-test-tier", tier)], tier)
                }),
            );
        }
        for (_, path) in CONTROL_ROUTES {
            for binding in [false, true] {
                let mut call = request(Method::POST, path, "owner");
                if binding {
                    call.extensions_mut()
                        .insert(layer1_public::AuthMethod::BindingKey {
                            binding_id: "binding-fixture".into(),
                        });
                }
                let response = app.clone().oneshot(call).await.unwrap();
                assert_eq!(
                    to_bytes(response.into_body(), 32).await.unwrap(),
                    "control",
                    "{path}, enabled={enabled}, binding={binding}"
                );
            }
            for method in [
                Method::GET,
                Method::HEAD,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
                Method::OPTIONS,
            ] {
                let response = app
                    .clone()
                    .oneshot(request(method.clone(), path, "owner"))
                    .await
                    .unwrap();
                let expected = if matches!(method, Method::GET | Method::HEAD)
                    && (path.ends_with("/controls") || path.ends_with("/queued-cancellation"))
                {
                    "recovery"
                } else {
                    "default"
                };
                assert_eq!(response.headers()["x-test-tier"], expected, "{path}");
            }
        }
    }
}

#[tokio::test]
async fn recovery_budget_is_separate_from_observation_input_and_control() {
    for enabled in [false, true] {
        let quota = Quota::per_hour(NonZeroU32::MIN);
        let mut limits = RateLimiters::from_quotas(
            quota,
            quota,
            quota,
            quota,
            quota,
            quota,
            TrustedProxyConfig::sample(),
        );
        limits.control_tier_enabled = enabled;
        let app = Router::new()
            .route("/api/v1/agents", get(|| async { StatusCode::OK }))
            .route(
                "/api/v2/agents/{agent_id}/queued-inputs",
                get(|| async { StatusCode::OK }),
            )
            .route(
                "/api/v2/agents/{agent_id}/responses/receipt",
                post(|| async { StatusCode::OK }),
            )
            .route(
                "/api/v1/agents/{id}/stop",
                post(|| async { StatusCode::OK }),
            )
            .route(
                "/api/v1/agents/{id}/input",
                post(|| async { StatusCode::OK }),
            )
            .layer(middleware::from_fn_with_state(Arc::new(limits), enforce));
        let send = |method, path| app.clone().oneshot(request(method, path, "owner"));
        let general = || send(Method::GET, "/api/v1/agents");
        assert_eq!(general().await.unwrap().status(), StatusCode::OK);
        refusal(general().await.unwrap()).await;
        let queue = send(Method::GET, "/api/v2/agents/agt_one/queued-inputs")
            .await
            .unwrap();
        if enabled {
            assert_eq!(queue.status(), StatusCode::OK);
            refusal(
                send(Method::POST, "/api/v2/agents/agt_one/responses/receipt")
                    .await
                    .unwrap(),
            )
            .await;
        } else {
            refusal(queue).await;
        }
        for path in [
            "/api/v1/agents/agt_one/stop",
            "/api/v1/agents/agt_one/input",
        ] {
            assert_eq!(
                send(Method::POST, path).await.unwrap().status(),
                StatusCode::OK
            );
        }
    }
}

#[tokio::test]
async fn unregistered_and_anonymous_recovery_requests_keep_the_original_budget() {
    let path = "/api/v2/agents/agt_one/queued-inputs";
    assert_eq!(
        request_tier(&request(Method::GET, path, "owner"), true, true),
        RequestTier::Default,
    );
    let route = Router::new().route(
        "/api/v2/agents/{agent_id}/queued-inputs",
        get(|request: Request| async move { request_tier(&request, true, true).name() }),
    );
    let call = Request::builder().uri(path).body(Body::empty()).unwrap();
    let response = route.oneshot(call).await.unwrap();
    assert_eq!(to_bytes(response.into_body(), 32).await.unwrap(), "default");
}

#[tokio::test]
async fn unregistered_controls_and_near_matches_remain_create() {
    for enabled in [false, true] {
        for (template, concrete) in CONTROL_ROUTES {
            for path in [*template, *concrete] {
                assert_eq!(
                    request_tier(&request(Method::POST, path, "owner"), enabled, true),
                    RequestTier::Create,
                    "no MatchedPath: {path}"
                );
            }
        }
        for template in [
            "/api/v1/agents/{agent_id}/stop",
            "/api/v1/agents/{id}/stop/extra",
            "/api/v1/agents/{id}/stop/",
            "/api/v2/agents/{id}/stop",
            "/api/v1/not-agents/{id}/stop",
            "/api/v1/agents/{id}/capacity-recovery/admit",
            "/api/v1/agents/{agent_id}/capacity-recovery/resume",
            "/api/v1/sandboxes/{id}/revive/extra",
            "/api/v1/sandboxes/{id}/workspaces",
            "/api/v1/workspaces/{id}/agents",
            "/api/v1/conversations",
            "/api/v2/agents/{id}/responses/{response_id}/controls",
            "/api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation/extra",
        ] {
            let app = Router::new().route(
                template,
                post(move |request: Request| async move {
                    request_tier(&request, enabled, true).name()
                }),
            );
            let path = template
                .replace("{agent_id}", "agt_one")
                .replace("{response_id}", "rsp_one")
                .replace("{request_id}", "req_one")
                .replace("{id}", "one");
            let response = app
                .oneshot(request(Method::POST, &path, "owner"))
                .await
                .unwrap();
            assert_eq!(
                to_bytes(response.into_body(), 32).await.unwrap(),
                "create",
                "{template}"
            );
        }
        for path in ["/livez", "/ready"] {
            for method in [Method::GET, Method::HEAD] {
                assert_eq!(
                    request_tier(&request(method, path, "owner"), enabled, true),
                    RequestTier::Health
                );
            }
            assert_eq!(
                request_tier(&request(Method::POST, path, "owner"), enabled, true),
                RequestTier::Create
            );
        }
    }
}

#[tokio::test]
async fn ten_creates_leave_twenty_controls_and_inputs_available_per_subject() {
    for enabled in [false, true] {
        let limits = local(enabled);
        let app = router(limits.clone());
        for _ in 0..10 {
            assert_eq!(
                app.clone()
                    .oneshot(request(Method::POST, "/api/v1/sandboxes", "owner"))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::CREATED
            );
        }
        refusal(
            app.clone()
                .oneshot(request(Method::POST, "/api/v1/sandboxes", "owner"))
                .await
                .unwrap(),
        )
        .await;
        let mut latencies = Vec::new();
        for n in 0..20 {
            let started = Instant::now();
            assert_eq!(
                app.clone()
                    .oneshot(request(
                        Method::POST,
                        CONTROL_ROUTES[n % CONTROL_ROUTES.len()].1,
                        "owner"
                    ))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NO_CONTENT
            );
            latencies.push(started.elapsed().as_micros());
            // With the flag off, only the legacy /input policy is guaranteed.
            let input = if enabled {
                "/api/v2/responses"
            } else {
                "/api/v1/agents/agt_one/input"
            };
            assert_eq!(
                app.clone()
                    .oneshot(request(Method::POST, input, "owner"))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NO_CONTENT
            );
        }
        latencies.sort_unstable();
        eprintln!("local TC-076 rate middleware: input_flag={enabled}, controls=20/20, inputs=20/20, control_p99_us={}", latencies[19]);
        assert_eq!(limits.rejections.load(Ordering::Relaxed), 1);
        for owner in ["another", "third"] {
            assert_eq!(
                app.clone()
                    .oneshot(request(Method::POST, "/api/v1/sandboxes", owner))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::CREATED
            );
            assert_eq!(
                app.clone()
                    .oneshot(request(Method::POST, CONTROL_ROUTES[0].1, owner))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NO_CONTENT
            );
        }
        assert_eq!(
            app.oneshot(request(Method::GET, "/api/v1/agents", "owner"))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }
}

#[tokio::test]
async fn control_aliases_share_a_finite_budget_and_count_rejections_without_spending_creates() {
    let limits = local(false);
    let app = router(limits.clone());
    for n in 0..120 {
        assert_eq!(
            app.clone()
                .oneshot(request(
                    Method::POST,
                    CONTROL_ROUTES[n % CONTROL_ROUTES.len()].1,
                    "noise"
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    for (_, path) in CONTROL_ROUTES {
        refusal(
            app.clone()
                .oneshot(request(Method::POST, path, "noise"))
                .await
                .unwrap(),
        )
        .await;
    }
    assert_eq!(
        limits.rejections.load(Ordering::Relaxed),
        CONTROL_ROUTES.len() as u64
    );
    assert_eq!(
        app.clone()
            .oneshot(request(Method::POST, CONTROL_ROUTES[0].1, "other"))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    for _ in 0..10 {
        assert_eq!(
            app.clone()
                .oneshot(request(Method::POST, "/api/v1/sandboxes", "noise"))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
    }
    assert_eq!(
        app.oneshot(request(
            Method::POST,
            "/api/v1/agents/agt_one/input",
            "noise"
        ))
        .await
        .unwrap()
        .status(),
        StatusCode::NO_CONTENT
    );
}

#[path = "rate_limit_control_mongo_tests.rs"]
mod mongo;

#[path = "rate_limit_control_disabled_tests.rs"]
mod disabled;
