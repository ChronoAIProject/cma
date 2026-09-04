use axum::body::Body;
use layer2_core::SubjectId;
use std::{
    num::NonZeroU32,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use super::visitor::{visitor_quota_key, QuotaResource};
use super::*;

fn request(method: Method, path: &str) -> Request {
    Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap()
}

fn visitor_limiters() -> Arc<RateLimiters> {
    let quota = || Quota::per_minute(NonZeroU32::new(1_000).unwrap());
    Arc::new(RateLimiters::from_quotas(
        quota(),
        quota(),
        quota(),
        quota(),
        quota(),
        TrustedProxyConfig::sample(),
    ))
}

fn time_on_day(day: u64, seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(day * 86_400 + seconds)
}

#[test]
fn tiers_are_generic_http_work_classes() {
    assert_eq!(
        request_tier(&request(Method::GET, "/livez")),
        RequestTier::Health
    );
    assert_eq!(
        request_tier(&request(Method::GET, "/api/v1/resources")),
        RequestTier::Default
    );
    assert_eq!(
        request_tier(&request(Method::POST, "/api/v1/resources")),
        RequestTier::Create
    );
    assert_eq!(
        request_tier(&request(Method::POST, "/api/v1/resources/id/input")),
        RequestTier::Input
    );
}

#[test]
fn public_discovery_and_auth_keep_network_scoped_budgets() {
    for path in [
        "/openapi.json",
        "/api/v1/conformance",
        "/api/v1/triggers/public/pub_example",
        "/api/v1/triggers/public/pub_example/badge.svg",
        "/.well-known/agent-card.json",
        "/auth/login",
    ] {
        assert!(uses_client_operation_key(path), "{path}");
    }
    for path in [
        "/api/v1/resources",
        "/api/v1/triggers/public/pub_example/launches",
        "/api/v1/triggers/public/pub_example/badge.svg/extra",
    ] {
        assert!(!uses_client_operation_key(path), "{path}");
    }
}

#[test]
fn ipv6_privacy_addresses_share_a_prefix_budget() {
    let first = "2001:db8:1234:5678::1".parse().unwrap();
    let second = "2001:db8:1234:5678:ffff::2".parse().unwrap();
    assert_eq!(normalize_client_ip(first), normalize_client_ip(second));
}

#[test]
fn forwarded_addresses_require_a_trusted_peer() {
    let mut request = request(Method::GET, "/api/v1/conformance");
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 10], 443))));
    request.headers_mut().insert(
        "x-forwarded-for",
        "198.51.100.7, 192.0.2.9".parse().unwrap(),
    );

    let untrusted = TrustedProxyConfig::sample();
    assert_eq!(
        resolve_client(&request, &untrusted),
        ClientKey::Network("192.0.2.10".parse().unwrap())
    );

    let trusted = TrustedProxyConfig {
        cidrs: vec!["192.0.2.0/24".parse().unwrap()],
        forwarded_hops: 1,
        cloudflare_cidrs: Vec::new(),
    };
    assert_eq!(
        resolve_client(&request, &trusted),
        ClientKey::Network("198.51.100.7".parse().unwrap())
    );
}

#[test]
fn visitor_launches_use_the_subject_and_exact_daily_limit() {
    let limiters = visitor_limiters();
    let subject = SubjectId::new("visitor-a");
    let now = time_on_day(20_000, 12 * 60 * 60);
    assert_eq!(visitor_quota_key(&subject), "trigger-visitor:visitor-a");

    for _ in 0..5 {
        limiters
            .admit_visitor_launch(&subject, 5, now)
            .expect("five launches fit");
    }
    let exhausted = limiters
        .admit_visitor_launch(&subject, 5, now)
        .expect_err("the sixth launch is refused");
    assert_eq!(exhausted.code(), "resource_quota");
    assert_eq!(exhausted.resource, QuotaResource::VisitorLaunches);
    assert_eq!(exhausted.resource.as_str(), "visitor_launches");
    assert_eq!(exhausted.limit, 5);
    assert_eq!(exhausted.retry_after_seconds(now), 12 * 60 * 60);
    limiters
        .admit_visitor_launch(&SubjectId::new("visitor-b"), 5, now)
        .expect("subjects have independent counts");
}

#[test]
fn visitor_launch_admission_is_atomic_under_concurrency() {
    let limiters = visitor_limiters();
    let now = time_on_day(20_000, 1);
    let admitted = std::thread::scope(|scope| {
        let handles = (0..32)
            .map(|_| {
                let limiters = Arc::clone(&limiters);
                scope.spawn(move || {
                    limiters
                        .admit_visitor_launch(&SubjectId::new("visitor-race"), 5, now)
                        .is_ok()
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|admitted| *admitted)
            .count()
    });
    assert_eq!(admitted, 5);
}

#[test]
fn visitor_launch_limit_is_live_and_resets_at_utc_midnight() {
    let limiters = visitor_limiters();
    let subject = SubjectId::new("visitor-dynamic");
    let before_midnight = time_on_day(20_000, 86_399);

    for _ in 0..3 {
        limiters
            .admit_visitor_launch(&subject, 5, before_midnight)
            .unwrap();
    }
    let reduced = limiters
        .admit_visitor_launch(&subject, 2, before_midnight)
        .expect_err("a reduced live limit applies immediately");
    assert_eq!(reduced.limit, 2);
    assert_eq!(reduced.retry_after_seconds(before_midnight), 1);
    limiters
        .admit_visitor_launch(&subject, 4, before_midnight)
        .expect("raising the limit admits one more");
    assert!(limiters
        .admit_visitor_launch(&subject, 4, before_midnight)
        .is_err());
    limiters
        .admit_visitor_launch(&subject, 1, time_on_day(20_001, 0))
        .expect("the next UTC day has a fresh count");
}
