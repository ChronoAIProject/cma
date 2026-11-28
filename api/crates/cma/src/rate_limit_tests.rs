use super::client_network::normalize_client_ip;
use async_trait::async_trait;
use axum::body::Body;
use axum::extract::ConnectInfo;
use layer1_public::rest::{LaunchQuota, LaunchQuotaError};
use layer2_core::{
    ActivityQuery, ActivityRecord, ActivityService, ActivityStore, Authorization, Caller,
    Capability, Page, PlatformSettings, PlatformSettingsService, PlatformSettingsStore, StoreError,
    StoredPlatformSettings, SubjectId,
};
use std::net::SocketAddr;
use std::{
    num::NonZeroU32,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use super::visitor::visitor_quota_key;
use super::*;

#[path = "rate_limit_relay_tests.rs"]
mod relay_tests;

#[derive(Default)]
struct NoopActivity;

#[async_trait]
impl ActivityStore for NoopActivity {
    async fn append(&self, _record: &ActivityRecord) -> Result<(), StoreError> {
        Ok(())
    }

    async fn list(&self, _query: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        Ok(Page::new(Vec::new(), None))
    }
}

struct MutableSettings(Mutex<StoredPlatformSettings>);

impl MutableSettings {
    fn new(visitor_launch_limit: u32) -> Self {
        let mut settings = PlatformSettings::default();
        settings.limits.visitor_max_launches_per_day = visitor_launch_limit;
        Self(Mutex::new(StoredPlatformSettings {
            changes: Default::default(),
            revision: 1,
            settings,
        }))
    }

    fn set_visitor_launch_limit(&self, limit: u32) {
        let mut stored = self.0.lock().unwrap();
        stored.revision += 1;
        stored.settings.limits.visitor_max_launches_per_day = limit;
    }
}

#[async_trait]
impl PlatformSettingsStore for MutableSettings {
    async fn read(&self) -> Result<StoredPlatformSettings, StoreError> {
        Ok(self.0.lock().unwrap().clone())
    }

    async fn replace(
        &self,
        expected_revision: u64,
        settings: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, StoreError> {
        let mut stored = self.0.lock().unwrap();
        if stored.revision != expected_revision {
            return Err(StoreError::RevisionConflict {
                expected: expected_revision,
                actual: stored.revision,
            });
        }
        *stored = StoredPlatformSettings {
            changes: Default::default(),
            revision: expected_revision + 1,
            settings: settings.clone(),
        };
        Ok(stored.clone())
    }
}

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
        request_tier(
            &request(
                Method::POST,
                "/api/v1/profile-authoring/repositories/resolve"
            ),
            false,
            true
        ),
        RequestTier::Default
    );
    assert_eq!(
        request_tier(&request(Method::GET, "/livez"), false, true),
        RequestTier::Health
    );
    assert_eq!(
        request_tier(&request(Method::GET, "/api/v1/resources"), false, true),
        RequestTier::Default
    );
    assert_eq!(
        request_tier(&request(Method::POST, "/api/v1/resources"), false, true),
        RequestTier::Create
    );
    assert_eq!(
        request_tier(
            &request(Method::POST, "/api/v1/resources/id/input"),
            false,
            true
        ),
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

#[tokio::test]
async fn connection_polling_keeps_creation_available_and_users_have_separate_login_budgets() {
    use axum::{http::StatusCode, middleware, routing::post, Router};
    use tower::ServiceExt as _;

    let quota = |n| Quota::per_hour(NonZeroU32::new(n).unwrap());
    let limiters = Arc::new(RateLimiters::from_quotas(
        quota(100),
        quota(100),
        quota(40),
        quota(1),
        quota(100),
        quota(100),
        TrustedProxyConfig::sample(),
    ));
    let app = Router::new()
        .route("/auth/connections/check", post(|| async { StatusCode::OK }))
        .route("/auth/connections/start", post(|| async { StatusCode::OK }))
        .layer(middleware::from_fn_with_state(limiters, enforce));
    let call = |path: &str, owner: &str| {
        let mut r = request(Method::POST, path);
        r.extensions_mut()
            .insert(AuthenticatedSubject(SubjectId::new(owner)));
        app.clone().oneshot(r)
    };
    // A full minute of the browser's two-second polling must leave its login
    // creation available, even with a deliberately tiny creation allowance.
    for _ in 0..30 {
        assert_eq!(
            call("/auth/connections/check", "one")
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }
    assert_eq!(
        call("/auth/connections/start", "one")
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        call("/auth/connections/start", "one")
            .await
            .unwrap()
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        call("/auth/connections/start", "two")
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
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
            .expect("five launches fit")
            .commit();
    }
    let exhausted = limiters
        .admit_visitor_launch(&subject, 5, now)
        .err()
        .expect("the sixth launch is refused");
    assert_eq!(exhausted.code(), "resource_quota");
    assert_eq!(exhausted.resource(), "visitor_launches");
    assert_eq!(exhausted.limit(), 5);
    assert_eq!(exhausted.retry_after_seconds(now), 12 * 60 * 60);
    limiters
        .admit_visitor_launch(&SubjectId::new("visitor-b"), 5, now)
        .expect("subjects have independent counts")
        .commit();
}

#[test]
fn visitor_launch_reservation_releases_on_drop_and_commit_retains_usage() {
    let limiters = visitor_limiters();
    let subject = SubjectId::new("visitor-release");
    let now = time_on_day(20_000, 1);
    let reservation = limiters.admit_visitor_launch(&subject, 1, now).unwrap();
    assert!(limiters.admit_visitor_launch(&subject, 1, now).is_err());
    drop(reservation);

    limiters
        .admit_visitor_launch(&subject, 1, now)
        .unwrap()
        .commit();
    assert!(limiters.admit_visitor_launch(&subject, 1, now).is_err());
}

#[test]
fn visitor_launch_admission_is_atomic_under_concurrency() {
    let limiters = visitor_limiters();
    let now = time_on_day(20_000, 1);
    let reservations = std::thread::scope(|scope| {
        let handles = (0..32)
            .map(|_| {
                let limiters = Arc::clone(&limiters);
                scope.spawn(move || {
                    limiters
                        .admit_visitor_launch(&SubjectId::new("visitor-race"), 5, now)
                        .ok()
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .filter_map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(reservations.len(), 5);
    assert!(limiters
        .admit_visitor_launch(&SubjectId::new("visitor-race"), 5, now)
        .is_err());
    drop(reservations);
    assert!(limiters
        .admit_visitor_launch(&SubjectId::new("visitor-race"), 5, now)
        .is_ok());
}

#[test]
fn visitor_launch_limit_is_live_and_resets_at_utc_midnight() {
    let limiters = visitor_limiters();
    let subject = SubjectId::new("visitor-dynamic");
    let before_midnight = time_on_day(20_000, 86_399);

    for _ in 0..3 {
        limiters
            .admit_visitor_launch(&subject, 5, before_midnight)
            .unwrap()
            .commit();
    }
    let reduced = limiters
        .admit_visitor_launch(&subject, 2, before_midnight)
        .err()
        .expect("a reduced live limit applies immediately");
    assert_eq!(reduced.limit(), 2);
    assert_eq!(reduced.retry_after_seconds(before_midnight), 1);
    limiters
        .admit_visitor_launch(&subject, 4, before_midnight)
        .expect("raising the limit admits one more")
        .commit();
    assert!(limiters
        .admit_visitor_launch(&subject, 4, before_midnight)
        .is_err());
    limiters
        .admit_visitor_launch(&subject, 1, time_on_day(20_001, 0))
        .expect("the next UTC day has a fresh count")
        .commit();
}

#[tokio::test]
async fn cma_launch_quota_reads_live_settings_and_bypasses_only_nonvisitors() {
    let limiters = visitor_limiters();
    let settings_store = Arc::new(MutableSettings::new(1));
    let settings = Arc::new(PlatformSettingsService::new(
        settings_store.clone(),
        ActivityService::new(Arc::new(NoopActivity)),
    ));
    let now = time_on_day(20_000, 60);
    let quota = CmaLaunchQuota::with_clock(Arc::clone(&limiters), settings, move || now);
    let visitor = Caller::new(
        SubjectId::new("visitor-live"),
        Authorization::new([Capability::LaunchTriggers]),
    );

    let first = quota.reserve(&visitor).await.unwrap();
    let refusal = quota.reserve(&visitor).await.unwrap_err();
    assert!(matches!(
        refusal,
        LaunchQuotaError::ResourceQuota(refusal)
            if refusal.limit() == 1
                && refusal.reset_at() == time_on_day(20_001, 0)
    ));
    drop(first);

    let first = quota.reserve(&visitor).await.unwrap();
    settings_store.set_visitor_launch_limit(2);
    let second = quota.reserve(&visitor).await.unwrap();
    settings_store.set_visitor_launch_limit(1);
    assert!(matches!(
        quota.reserve(&visitor).await,
        Err(LaunchQuotaError::ResourceQuota(refusal)) if refusal.limit() == 1
    ));
    drop((first, second));

    settings_store.set_visitor_launch_limit(0);
    for authorization in [
        Authorization::new([Capability::LaunchTriggers, Capability::CreateSandbox]),
        Authorization::new([Capability::UseOwnSandboxes]),
    ] {
        let nonvisitor = Caller::new(SubjectId::new("not-a-visitor"), authorization);
        quota
            .reserve(&nonvisitor)
            .await
            .expect("non-visitors are unmetered");
    }
}
