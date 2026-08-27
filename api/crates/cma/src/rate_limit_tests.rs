use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    num::NonZeroU32,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use axum::{
    body::{to_bytes, Body},
    extract::{ConnectInfo, Request, State},
    http::{header, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::Response,
    routing::{get, post},
    Router,
};
use governor::Quota;
use layer1_public::AuthenticatedSubject;
use layer2_core::SubjectId;
use tower::ServiceExt;

use crate::{
    config::TrustedProxyConfig,
    rate_limit::{self, RateLimiters},
    with_cors, REST_LINKS,
};

const SUBJECT: &str = "subject-rate-limit";

fn quota(burst: u32, replenish_one_every: Duration) -> Quota {
    Quota::with_period(replenish_one_every)
        .expect("the test period is positive")
        .allow_burst(NonZeroU32::new(burst).expect("the test burst is positive"))
}

fn one() -> Quota {
    quota(1, Duration::from_secs(60))
}

fn generous() -> Quota {
    quota(1_000, Duration::from_millis(1))
}

fn test_routes() -> Router {
    Router::new()
        .route("/openapi.json", get(|| async { StatusCode::NO_CONTENT }))
        .route(
            "/.well-known/agent-card.json",
            get(|| async { StatusCode::NO_CONTENT }),
        )
        .route(
            "/api/v1/sessions",
            post(|| async { StatusCode::NO_CONTENT }),
        )
        .route(
            "/api/v1/sessions/{id}",
            get(|| async { StatusCode::NO_CONTENT }),
        )
        .route(
            "/api/v1/sessions/{id}/input",
            post(|| async { StatusCode::NO_CONTENT }),
        )
        .route(
            "/api/v1/sessions/{id}/steer",
            post(|| async { StatusCode::NO_CONTENT }),
        )
        .route(
            "/api/v1/sessions/{id}/plane",
            post(|| async { StatusCode::NO_CONTENT }),
        )
        .route(
            "/api/v1/sessions/{id}/revive",
            post(|| async { StatusCode::NO_CONTENT }),
        )
        .route(
            "/api/v1/launches/{id}/start",
            post(|| async { StatusCode::NO_CONTENT }),
        )
        .route(
            "/api/v1/triggers/public/{public_id}",
            get(|| async { StatusCode::NO_CONTENT }),
        )
        .route(
            "/api/v1/triggers/public/{public_id}/launches",
            post(|| async { StatusCode::NO_CONTENT }),
        )
        .route(
            "/api/v1/agui/triggers/{public_id}",
            get(|| async { StatusCode::NO_CONTENT }),
        )
        .route(
            "/api/v1/agui/run",
            post(|| async { StatusCode::NO_CONTENT }),
        )
        .route("/api/v1/agui", get(|| async { StatusCode::NO_CONTENT }))
        .route(
            "/api/v1/agui/unknown",
            get(|| async { StatusCode::NO_CONTENT }),
        )
        .route(
            "/api/v1/agui/capabilities",
            get(|| async { StatusCode::NO_CONTENT }),
        )
        .route(
            "/api/v1/sessions/{id}/terminal",
            get(|| async { StatusCode::NO_CONTENT }),
        )
        .route(
            "/api/v1/sessions/{id}/terminal/",
            get(|| async { StatusCode::NO_CONTENT }),
        )
        .route("/api/v1/mcp", post(|| async { StatusCode::NO_CONTENT }))
        .route("/api/v1/a2a", post(|| async { StatusCode::NO_CONTENT }))
        .route(
            "/api/v1/conformance",
            get(|| async { StatusCode::NO_CONTENT }),
        )
        .route("/auth", get(|| async { StatusCode::NO_CONTENT }))
        .route("/auth/callback", get(|| async { StatusCode::NO_CONTENT }))
        .route("/livez", get(|| async { StatusCode::NO_CONTENT }))
        .route("/ready", get(|| async { StatusCode::NO_CONTENT }))
}

fn operation_app(limiters: Arc<RateLimiters>) -> Router {
    test_routes().layer(axum::middleware::from_fn_with_state(
        limiters,
        rate_limit::enforce,
    ))
}

fn guarded_app(limiters: Arc<RateLimiters>) -> Router {
    test_routes()
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&limiters),
            rate_limit::enforce,
        ))
        .layer(axum::middleware::from_fn_with_state(
            limiters,
            rate_limit::admit,
        ))
}

fn request(
    method: Method,
    path: &str,
    subject: Option<&str>,
    peer: Option<IpAddr>,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .expect("test request builds");
    if let Some(subject) = subject {
        request
            .extensions_mut()
            .insert(AuthenticatedSubject(SubjectId(subject.to_owned())));
    }
    if let Some(peer) = peer {
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(peer, 4242)));
    }
    request
}

fn subject_request(method: Method, path: &str) -> Request<Body> {
    request(method, path, Some(SUBJECT), None)
}

fn with_xff(mut request: Request<Body>, value: &'static str) -> Request<Body> {
    request
        .headers_mut()
        .insert("x-forwarded-for", HeaderValue::from_static(value));
    request
}

fn trusted_proxy(cidrs: &[&str], forwarded_hops: usize) -> TrustedProxyConfig {
    TrustedProxyConfig {
        cidrs: cidrs
            .iter()
            .map(|cidr| cidr.parse().expect("test CIDR parses"))
            .collect(),
        forwarded_hops,
    }
}

fn complete_limiters(
    admission: Quota,
    health: Quota,
    default: Quota,
    create: Quota,
    input: Quota,
    trusted: TrustedProxyConfig,
) -> Arc<RateLimiters> {
    RateLimiters::for_test_complete(admission, health, default, create, input, trusted)
}

async fn status(app: &Router, request: Request<Body>) -> StatusCode {
    app.clone()
        .oneshot(request)
        .await
        .expect("router answers")
        .status()
}

#[tokio::test]
async fn a_create_burst_is_coded_linked_and_recovers_after_retry_after() {
    let generous = quota(20, Duration::from_millis(10));
    let app = operation_app(RateLimiters::for_test(
        generous,
        quota(2, Duration::from_secs(1)),
        quota(20, Duration::from_millis(10)),
    ));

    for _ in 0..2 {
        assert_eq!(
            status(&app, subject_request(Method::POST, "/api/v1/sessions")).await,
            StatusCode::NO_CONTENT
        );
    }
    let response = app
        .clone()
        .oneshot(subject_request(Method::POST, "/api/v1/sessions"))
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE),
        Some(&HeaderValue::from_static("application/problem+json"))
    );
    assert_eq!(response.headers().get(header::LINK), Some(&REST_LINKS));
    let retry_after = response
        .headers()
        .get(header::RETRY_AFTER)
        .expect("429 carries Retry-After")
        .to_str()
        .expect("Retry-After is text")
        .parse::<u64>()
        .expect("Retry-After is a whole number");
    assert!(retry_after > 0);
    let body: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("problem body is bounded"),
    )
    .expect("problem body is json");
    assert_eq!(body["code"], "rate_limited");
    assert_eq!(body["status"], 429);

    tokio::time::sleep(Duration::from_secs(retry_after)).await;
    assert_eq!(
        status(&app, subject_request(Method::POST, "/api/v1/sessions")).await,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn every_provisioning_and_prompt_route_uses_its_strict_semantic_tier() {
    for path in [
        "/api/v1/sessions",
        "/api/v1/agui/run",
        "/api/v1/sessions/sesn_rate/plane",
        "/api/v1/sessions/sesn_rate/revive",
        "/api/v1/launches/launch_rate/start",
    ] {
        let app = operation_app(RateLimiters::for_test(generous(), one(), generous()));
        assert_eq!(
            status(&app, subject_request(Method::POST, path)).await,
            StatusCode::NO_CONTENT,
            "first provisioning operation at {path}"
        );
        assert_eq!(
            status(&app, subject_request(Method::POST, path)).await,
            StatusCode::TOO_MANY_REQUESTS,
            "second provisioning operation at {path}"
        );
    }

    for path in [
        "/api/v1/sessions/sesn_rate/input",
        "/api/v1/sessions/sesn_rate/steer",
    ] {
        let app = operation_app(RateLimiters::for_test(generous(), generous(), one()));
        assert_eq!(
            status(&app, subject_request(Method::POST, path)).await,
            StatusCode::NO_CONTENT,
            "first prompt operation at {path}"
        );
        assert_eq!(
            status(&app, subject_request(Method::POST, path)).await,
            StatusCode::TOO_MANY_REQUESTS,
            "second prompt operation at {path}"
        );
    }
}

#[tokio::test]
async fn default_create_and_input_budgets_are_independent() {
    let app = operation_app(RateLimiters::for_test(one(), one(), one()));
    for (method, path) in [
        (Method::POST, "/api/v1/sessions"),
        (Method::POST, "/api/v1/sessions/sesn_rate/input"),
        (Method::GET, "/api/v1/conformance"),
    ] {
        assert_eq!(
            status(&app, subject_request(method.clone(), path)).await,
            StatusCode::NO_CONTENT,
            "the first {method} {path} should use its own budget"
        );
        assert_eq!(
            status(&app, subject_request(method.clone(), path)).await,
            StatusCode::TOO_MANY_REQUESTS,
            "the second {method} {path} should exhaust only its own budget"
        );
    }
}

#[tokio::test]
async fn public_routes_use_network_keys_even_with_ambient_subjects() {
    let peer_a = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10));
    let peer_b = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 11));
    for (method, path) in [
        (Method::GET, "/openapi.json"),
        (Method::GET, "/api/v1/conformance"),
        (Method::GET, "/api/v1/agui/capabilities"),
        (
            Method::GET,
            "/api/v1/triggers/public/123e4567-e89b-12d3-a456-426614174000",
        ),
        (
            Method::GET,
            "/api/v1/agui/triggers/123e4567-e89b-12d3-a456-426614174000",
        ),
        (Method::POST, "/api/v1/mcp"),
        (Method::POST, "/api/v1/a2a"),
        (Method::GET, "/.well-known/agent-card.json"),
        (Method::GET, "/auth"),
        (Method::GET, "/auth/callback"),
    ] {
        let app = operation_app(RateLimiters::for_test(one(), one(), one()));
        assert_eq!(
            status(&app, request(method.clone(), path, None, Some(peer_a))).await,
            StatusCode::NO_CONTENT,
            "first public request to {path}"
        );
        assert_eq!(
            status(
                &app,
                request(method.clone(), path, Some(SUBJECT), Some(peer_a))
            )
            .await,
            StatusCode::TOO_MANY_REQUESTS,
            "ambient identity moved {path} off the client budget"
        );
        assert_eq!(
            status(&app, request(method, path, Some(SUBJECT), Some(peer_b))).await,
            StatusCode::NO_CONTENT,
            "one client network consumed another at {path}"
        );
        assert_eq!(
            status(
                &app,
                request(
                    Method::GET,
                    "/api/v1/sessions/sesn_rate",
                    Some(SUBJECT),
                    Some(peer_a),
                )
            )
            .await,
            StatusCode::NO_CONTENT,
            "public request to {path} consumed the subject budget"
        );
        assert_eq!(
            status(
                &app,
                request(
                    Method::GET,
                    "/api/v1/sessions/sesn_rate",
                    Some(SUBJECT),
                    Some(peer_b),
                )
            )
            .await,
            StatusCode::TOO_MANY_REQUESTS,
            "protected subject budget split across networks after {path}"
        );
    }
}

#[tokio::test]
async fn authenticated_launch_creation_nested_under_public_preview_is_subject_keyed() {
    let app = operation_app(RateLimiters::for_test(one(), generous(), generous()));
    let path = "/api/v1/triggers/public/123e4567-e89b-12d3-a456-426614174000/launches";
    assert_eq!(
        status(
            &app,
            request(
                Method::POST,
                path,
                Some(SUBJECT),
                Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10))),
            ),
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        status(
            &app,
            request(
                Method::POST,
                path,
                Some(SUBJECT),
                Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 11))),
            ),
        )
        .await,
        StatusCode::TOO_MANY_REQUESTS,
        "a new network must not mint another budget for the same authenticated subject"
    );
}

#[tokio::test]
async fn forwarding_headers_are_used_only_from_a_trusted_peer_and_skip_spoofed_prefixes() {
    let trusted = trusted_proxy(&["35.191.0.0/16"], 1);
    let app = operation_app(complete_limiters(
        generous(),
        generous(),
        one(),
        generous(),
        generous(),
        trusted,
    ));
    let gfe = "35.191.4.7".parse().unwrap();

    let first = with_xff(
        request(Method::GET, "/api/v1/conformance", None, Some(gfe)),
        "203.0.113.200, 198.51.100.10, 192.0.2.40",
    );
    assert_eq!(status(&app, first).await, StatusCode::NO_CONTENT);

    let same_client_new_spoof = with_xff(
        request(Method::GET, "/api/v1/conformance", None, Some(gfe)),
        "203.0.113.201, 198.51.100.10, 192.0.2.40",
    );
    assert_eq!(
        status(&app, same_client_new_spoof).await,
        StatusCode::TOO_MANY_REQUESTS,
        "a caller-prepended token must not shard the selected client"
    );

    let different_client = with_xff(
        request(Method::GET, "/api/v1/conformance", None, Some(gfe)),
        "203.0.113.200, 198.51.100.11, 192.0.2.40",
    );
    assert_eq!(status(&app, different_client).await, StatusCode::NO_CONTENT);

    let untrusted_app = operation_app(RateLimiters::for_test(one(), generous(), generous()));
    let peer = "192.0.2.90".parse().unwrap();
    let spoof_a = with_xff(
        request(Method::GET, "/api/v1/conformance", None, Some(peer)),
        "198.51.100.20, 192.0.2.40",
    );
    let spoof_b = with_xff(
        request(Method::GET, "/api/v1/conformance", None, Some(peer)),
        "198.51.100.21, 192.0.2.40",
    );
    assert_eq!(
        status(&untrusted_app, spoof_a).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        status(&untrusted_app, spoof_b).await,
        StatusCode::TOO_MANY_REQUESTS,
        "an untrusted peer's header must never change its key"
    );
}

#[tokio::test]
async fn unusable_trusted_forwarding_chains_fall_back_to_the_tcp_peer() {
    let gfe = "35.191.4.7".parse().unwrap();
    for (description, first_header, second_header) in [
        ("missing", None, None),
        (
            "malformed",
            Some("not-an-ip, 192.0.2.40"),
            Some("still-not-an-ip, 192.0.2.41"),
        ),
        (
            "empty token",
            Some("198.51.100.10, , 192.0.2.40"),
            Some("198.51.100.11, , 192.0.2.41"),
        ),
        ("too short", Some("192.0.2.40"), Some("192.0.2.41")),
    ] {
        let app = operation_app(complete_limiters(
            generous(),
            generous(),
            one(),
            generous(),
            generous(),
            trusted_proxy(&["35.191.0.0/16"], 1),
        ));
        let first = request(Method::GET, "/api/v1/conformance", None, Some(gfe));
        let second = request(Method::GET, "/api/v1/conformance", None, Some(gfe));
        let first = match first_header {
            Some(value) => with_xff(first, value),
            None => first,
        };
        let second = match second_header {
            Some(value) => with_xff(second, value),
            None => second,
        };

        assert_eq!(status(&app, first).await, StatusCode::NO_CONTENT);
        assert_eq!(
            status(&app, second).await,
            StatusCode::TOO_MANY_REQUESTS,
            "{description} chain did not share the TCP-peer fallback"
        );
    }
}

#[tokio::test]
async fn multiple_forwarding_header_fields_form_one_right_anchored_chain() {
    let app = operation_app(complete_limiters(
        generous(),
        generous(),
        one(),
        generous(),
        generous(),
        trusted_proxy(&["35.191.0.0/16"], 1),
    ));
    let gfe = "35.191.4.7".parse().unwrap();
    for (spoofed, expected) in [
        ("203.0.113.10", StatusCode::NO_CONTENT),
        ("203.0.113.11", StatusCode::TOO_MANY_REQUESTS),
    ] {
        let mut request = request(Method::GET, "/api/v1/conformance", None, Some(gfe));
        request
            .headers_mut()
            .append("x-forwarded-for", HeaderValue::from_static(spoofed));
        request.headers_mut().append(
            "x-forwarded-for",
            HeaderValue::from_static("198.51.100.10, 192.0.2.40"),
        );
        assert_eq!(
            status(&app, request).await,
            expected,
            "caller prefix in another header field changed the client key"
        );
    }
}

#[tokio::test]
async fn ipv6_privacy_addresses_share_a_64_and_mapped_addresses_share_ipv4() {
    let app = operation_app(RateLimiters::for_test(one(), generous(), generous()));
    let first = "2001:db8:10:20::1".parse().unwrap();
    let same_prefix = "2001:db8:10:20:ffff::99".parse().unwrap();
    let other_prefix = "2001:db8:10:21::1".parse().unwrap();

    assert_eq!(
        status(
            &app,
            request(Method::GET, "/api/v1/conformance", None, Some(first))
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        status(
            &app,
            request(Method::GET, "/api/v1/conformance", None, Some(same_prefix),)
        )
        .await,
        StatusCode::TOO_MANY_REQUESTS,
        "privacy addresses in one /64 must share a budget"
    );
    assert_eq!(
        status(
            &app,
            request(Method::GET, "/api/v1/conformance", None, Some(other_prefix),)
        )
        .await,
        StatusCode::NO_CONTENT,
        "different /64 networks keep separate budgets"
    );

    let mapped_app = operation_app(RateLimiters::for_test(one(), generous(), generous()));
    let mapped = "::ffff:192.0.2.33".parse().unwrap();
    let ipv4 = "192.0.2.33".parse().unwrap();
    assert_eq!(
        status(
            &mapped_app,
            request(Method::GET, "/api/v1/conformance", None, Some(mapped))
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        status(
            &mapped_app,
            request(Method::GET, "/api/v1/conformance", None, Some(ipv4))
        )
        .await,
        StatusCode::TOO_MANY_REQUESTS,
        "mapped and native IPv4 forms must be one key"
    );
}

#[tokio::test]
async fn requests_without_peer_metadata_share_one_fail_closed_key() {
    let app = operation_app(RateLimiters::for_test(one(), generous(), generous()));
    assert_eq!(
        status(
            &app,
            request(Method::GET, "/api/v1/conformance", None, None)
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        status(
            &app,
            request(Method::GET, "/api/v1/conformance", None, None)
        )
        .await,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn health_and_network_admission_budgets_are_independent() {
    let app = guarded_app(complete_limiters(
        one(),
        one(),
        generous(),
        generous(),
        generous(),
        TrustedProxyConfig::sample(),
    ));
    let peer = "192.0.2.61".parse().unwrap();

    assert_eq!(
        status(
            &app,
            request(Method::GET, "/api/v1/conformance", None, Some(peer))
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        status(
            &app,
            request(Method::GET, "/api/v1/conformance", None, Some(peer))
        )
        .await,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        status(&app, request(Method::GET, "/ready", None, Some(peer))).await,
        StatusCode::NO_CONTENT,
        "caller traffic must not consume the probe budget"
    );
    assert_eq!(
        status(&app, request(Method::GET, "/livez", None, Some(peer))).await,
        StatusCode::TOO_MANY_REQUESTS,
        "health endpoints deliberately share their protected budget"
    );
}

#[tokio::test]
async fn head_health_requests_share_the_get_probe_budget() {
    let app = guarded_app(complete_limiters(
        generous(),
        one(),
        generous(),
        generous(),
        generous(),
        TrustedProxyConfig::sample(),
    ));
    let peer = "192.0.2.62".parse().unwrap();
    assert_eq!(
        status(&app, request(Method::HEAD, "/ready", None, Some(peer))).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        status(&app, request(Method::GET, "/livez", None, Some(peer))).await,
        StatusCode::TOO_MANY_REQUESTS,
        "HEAD and GET probes did not share the health budget"
    );
}

#[tokio::test]
async fn wrong_method_health_paths_use_the_ordinary_operation_budget() {
    let app = operation_app(RateLimiters::for_test(one(), generous(), generous()));
    let first = status(&app, subject_request(Method::POST, "/ready")).await;
    assert_ne!(first, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        status(&app, subject_request(Method::POST, "/ready")).await,
        StatusCode::TOO_MANY_REQUESTS,
        "a wrong-method health path bypassed the ordinary budget"
    );
}

async fn count_inner(
    State(calls): State<Arc<AtomicUsize>>,
    request: Request,
    next: Next,
) -> Response {
    calls.fetch_add(1, Ordering::SeqCst);
    next.run(request).await
}

#[tokio::test]
async fn production_order_rejects_before_auth_body_activity_and_handler_work() {
    let admission_calls = Arc::new(AtomicUsize::new(0));
    let admission_limiters = complete_limiters(
        one(),
        generous(),
        generous(),
        generous(),
        generous(),
        TrustedProxyConfig::sample(),
    );
    let admission_app = Router::new()
        .route(
            "/api/v1/conformance",
            get(|| async { StatusCode::NO_CONTENT }),
        )
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&admission_calls),
            count_inner,
        ))
        .layer(axum::middleware::from_fn_with_state(
            admission_limiters,
            rate_limit::admit,
        ));
    let peer = "192.0.2.71".parse().unwrap();
    for expected in [StatusCode::NO_CONTENT, StatusCode::TOO_MANY_REQUESTS] {
        assert_eq!(
            status(
                &admission_app,
                request(Method::GET, "/api/v1/conformance", None, Some(peer))
            )
            .await,
            expected
        );
    }
    assert_eq!(
        admission_calls.load(Ordering::SeqCst),
        1,
        "pre-auth rejection must not enter any inner work"
    );

    let activity_calls = Arc::new(AtomicUsize::new(0));
    let operation_app = Router::new()
        .route(
            "/api/v1/sessions",
            post(|| async { StatusCode::NO_CONTENT }),
        )
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&activity_calls),
            count_inner,
        ))
        .layer(axum::middleware::from_fn_with_state(
            RateLimiters::for_test(generous(), one(), generous()),
            rate_limit::enforce,
        ));
    for expected in [StatusCode::NO_CONTENT, StatusCode::TOO_MANY_REQUESTS] {
        assert_eq!(
            status(
                &operation_app,
                subject_request(Method::POST, "/api/v1/sessions")
            )
            .await,
            expected
        );
    }
    assert_eq!(
        activity_calls.load(Ordering::SeqCst),
        1,
        "an exact rejection must not enter body capture or activity recording"
    );
}

#[tokio::test]
async fn cors_preflights_are_exempt_and_retry_after_is_browser_visible() {
    let origin = HeaderValue::from_static("https://integrator.example");
    let app = with_cors(
        guarded_app(complete_limiters(
            one(),
            generous(),
            one(),
            generous(),
            generous(),
            TrustedProxyConfig::sample(),
        )),
        vec![origin.clone()],
    );
    let peer = IpAddr::V4(Ipv4Addr::LOCALHOST);

    for _ in 0..2 {
        let mut preflight = request(Method::OPTIONS, "/api/v1/conformance", None, Some(peer));
        preflight
            .headers_mut()
            .insert(header::ORIGIN, origin.clone());
        preflight.headers_mut().insert(
            header::ACCESS_CONTROL_REQUEST_METHOD,
            HeaderValue::from_static("GET"),
        );
        assert!(
            status(&app, preflight).await.is_success(),
            "CORS should answer a listed preflight"
        );
    }

    let mut admitted = request(Method::GET, "/api/v1/conformance", None, Some(peer));
    admitted
        .headers_mut()
        .insert(header::ORIGIN, origin.clone());
    assert_eq!(status(&app, admitted).await, StatusCode::NO_CONTENT);

    let mut limited = request(Method::GET, "/api/v1/conformance", None, Some(peer));
    limited.headers_mut().insert(header::ORIGIN, origin);
    let response = app.oneshot(limited).await.expect("router answers");
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let exposed = response
        .headers()
        .get(header::ACCESS_CONTROL_EXPOSE_HEADERS)
        .expect("CORS exposes response headers")
        .to_str()
        .expect("exposed header names are text")
        .to_ascii_lowercase();
    assert!(
        exposed.split(',').any(|name| name.trim() == "retry-after"),
        "Retry-After was not exposed: {exposed}"
    );
}

#[tokio::test]
async fn arbitrary_options_requests_are_not_exempt_as_cors_preflights() {
    let origin = HeaderValue::from_static("https://integrator.example");
    let app = with_cors(
        guarded_app(complete_limiters(
            generous(),
            generous(),
            one(),
            generous(),
            generous(),
            TrustedProxyConfig::sample(),
        )),
        vec![origin],
    );
    let peer = IpAddr::V4(Ipv4Addr::LOCALHOST);

    let first = status(
        &app,
        request(Method::OPTIONS, "/api/v1/conformance", None, Some(peer)),
    )
    .await;
    assert_ne!(first, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        status(
            &app,
            request(Method::OPTIONS, "/api/v1/conformance", None, Some(peer))
        )
        .await,
        StatusCode::TOO_MANY_REQUESTS,
        "OPTIONS without the preflight headers consumes the ordinary budget"
    );
}

#[tokio::test]
async fn cors_preflights_grant_only_declared_methods_and_headers() {
    let origin = HeaderValue::from_static("https://integrator.example");
    let app = with_cors(test_routes(), vec![origin.clone()]);

    for (method, requested_headers) in [("CONNECT", None), ("GET", Some("x-undeclared"))] {
        let mut preflight = request(
            Method::OPTIONS,
            "/api/v1/conformance",
            None,
            Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        );
        preflight
            .headers_mut()
            .insert(header::ORIGIN, origin.clone());
        preflight.headers_mut().insert(
            header::ACCESS_CONTROL_REQUEST_METHOD,
            HeaderValue::from_static(method),
        );
        if let Some(requested_headers) = requested_headers {
            preflight.headers_mut().insert(
                header::ACCESS_CONTROL_REQUEST_HEADERS,
                HeaderValue::from_static(requested_headers),
            );
        }

        let response = app.clone().oneshot(preflight).await.expect("CORS answers");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none(),
            "undeclared preflight shape received a browser grant"
        );
    }
}

#[tokio::test]
async fn only_rest_rejections_claim_the_rest_profile() {
    for (method, path, carries_rest_links) in [
        (Method::GET, "/openapi.json", true),
        (Method::GET, "/api/v1/sessions/sesn_rate", true),
        (Method::GET, "/api/v1/agui", false),
        (Method::GET, "/api/v1/agui/unknown", false),
        (Method::POST, "/api/v1/agui/run", false),
        (Method::GET, "/api/v1/agui/capabilities", false),
        (Method::GET, "/api/v1/sessions/sesn_rate/terminal", false),
        (Method::GET, "/api/v1/sessions/sesn_rate/terminal/", true),
        (Method::POST, "/api/v1/mcp", false),
        (Method::POST, "/api/v1/a2a", false),
        (Method::GET, "/.well-known/agent-card.json", false),
    ] {
        let app = operation_app(RateLimiters::for_test(one(), one(), one()));
        assert_eq!(
            status(&app, subject_request(method.clone(), path)).await,
            StatusCode::NO_CONTENT,
            "first request to {path}"
        );
        let response = app
            .oneshot(subject_request(method, path))
            .await
            .expect("router answers");
        assert_eq!(
            response.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "for {path}"
        );
        assert_eq!(
            response.headers().get(header::LINK),
            carries_rest_links.then_some(&REST_LINKS),
            "REST Link classification for {path}"
        );
    }
}
