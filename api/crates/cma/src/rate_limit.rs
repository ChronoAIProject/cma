//! Per-caller protection for the public control-plane surface.
//!
//! Limiting has two stages. A cheap per-network admission guard runs before
//! authentication, body capture, and durable activity recording so hostile
//! traffic cannot make the work intended to protect unbounded. After identity
//! is established, an operation limiter applies the documented per-subject
//! budgets; anonymous requests use the same resolved client network as the
//! admission guard. Rejections are counted and logged at exponentially sparse
//! intervals instead of being written to MongoDB, because awaiting MongoDB for
//! traffic refused to protect MongoDB would defeat the boundary.
//!
//! The TCP peer is authoritative by default. `X-Forwarded-For` is considered
//! only when that peer belongs to an explicitly configured trusted proxy
//! network, and a fixed number of rightmost proxy tokens is skipped. This
//! follows the Google external Application Load Balancer contract without
//! believing caller-prepended values. IPv4-mapped IPv6 is canonicalized to
//! IPv4; ordinary IPv6 clients share a `/64`, so rotating privacy addresses
//! does not mint fresh budgets.
//!
//! Counters live in this process. That is exact while the deployment is pinned
//! to one replica. Running N replicas makes the effective allowance up to N
//! times larger, so scale-out must first choose a shared limiter if configured
//! numbers must remain deployment-wide.

use std::{
    net::{IpAddr, Ipv6Addr, SocketAddr},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use axum::{
    extract::{ConnectInfo, MatchedPath, Request, State},
    http::{header, HeaderMap, Method},
    middleware::Next,
    response::{IntoResponse, Response},
};
use governor::{clock::Clock, DefaultKeyedRateLimiter, Quota, RateLimiter};
use layer1_public::{rest::HttpProblem, AuthenticatedSubject};

use crate::{
    config::{RateLimitConfig, TrustedProxyConfig},
    is_cors_preflight, REST_LINKS,
};

const RETAIN_RECENT_EVERY: u64 = 1024;

type KeyedLimiter = DefaultKeyedRateLimiter<RateKey>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ClientKey {
    Network(IpAddr),
    Unidentified,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum RateKey {
    Subject(String),
    Client(ClientKey),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestTier {
    Health,
    Default,
    Create,
    Input,
}

impl RequestTier {
    fn name(self) -> &'static str {
        match self {
            Self::Health => "health",
            Self::Default => "default",
            Self::Create => "create",
            Self::Input => "input",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Budget {
    Admission,
    Health,
    Default,
    Create,
    Input,
}

/// Independent admission, health, and operation budgets.
pub(crate) struct RateLimiters {
    admission: KeyedLimiter,
    health: KeyedLimiter,
    default: KeyedLimiter,
    create: KeyedLimiter,
    input: KeyedLimiter,
    trusted_proxies: TrustedProxyConfig,
    decisions: AtomicU64,
    rejections: AtomicU64,
}

impl RateLimiters {
    /// Build production budgets from validated configuration.
    pub(crate) fn from_config(
        config: RateLimitConfig,
        trusted_proxies: TrustedProxyConfig,
    ) -> Arc<Self> {
        Arc::new(Self::from_quotas(
            Quota::per_minute(config.admission_per_minute),
            Quota::per_minute(config.health_per_minute),
            Quota::per_minute(config.default_per_minute),
            Quota::per_minute(config.create_per_minute),
            Quota::per_minute(config.input_per_minute),
            trusted_proxies,
        ))
    }

    fn from_quotas(
        admission: Quota,
        health: Quota,
        default: Quota,
        create: Quota,
        input: Quota,
        trusted_proxies: TrustedProxyConfig,
    ) -> Self {
        Self {
            admission: RateLimiter::keyed(admission),
            health: RateLimiter::keyed(health),
            default: RateLimiter::keyed(default),
            create: RateLimiter::keyed(create),
            input: RateLimiter::keyed(input),
            trusted_proxies,
            decisions: AtomicU64::new(0),
            rejections: AtomicU64::new(0),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(default: Quota, create: Quota, input: Quota) -> Arc<Self> {
        Self::for_test_complete(
            generous_test_quota(),
            generous_test_quota(),
            default,
            create,
            input,
            TrustedProxyConfig::sample(),
        )
    }

    #[cfg(test)]
    pub(crate) fn for_test_complete(
        admission: Quota,
        health: Quota,
        default: Quota,
        create: Quota,
        input: Quota,
        trusted_proxies: TrustedProxyConfig,
    ) -> Arc<Self> {
        Arc::new(Self::from_quotas(
            admission,
            health,
            default,
            create,
            input,
            trusted_proxies,
        ))
    }

    fn check(&self, budget: Budget, key: &RateKey) -> Result<(), u64> {
        let limiter = match budget {
            Budget::Admission => &self.admission,
            Budget::Health => &self.health,
            Budget::Default => &self.default,
            Budget::Create => &self.create,
            Budget::Input => &self.input,
        };
        let decision = limiter
            .check_key(key)
            .map_err(|refusal| retry_after_seconds(refusal.wait_time_from(limiter.clock().now())));
        self.maybe_retain_recent();
        decision
    }

    fn maybe_retain_recent(&self) {
        let decision = self
            .decisions
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        if decision.is_multiple_of(RETAIN_RECENT_EVERY) {
            self.admission.retain_recent();
            self.health.retain_recent();
            self.default.retain_recent();
            self.create.retain_recent();
            self.input.retain_recent();
        }
    }

    fn note_rejection(&self, stage: &'static str, tier: RequestTier) {
        let total = self
            .rejections
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        if total.is_power_of_two() {
            tracing::warn!(
                rate_limit_rejections = total,
                stage,
                tier = tier.name(),
                "rate-limited requests are counted here and intentionally skip durable activity recording"
            );
        }
    }
}

/// Bound work before authentication or request-body capture occurs.
pub(crate) async fn admit(
    State(limiters): State<Arc<RateLimiters>>,
    mut request: Request,
    next: Next,
) -> Response {
    if is_cors_preflight(&request) {
        return next.run(request).await;
    }

    let tier = request_tier(&request);
    let client = resolve_client(&request, &limiters.trusted_proxies);
    request.extensions_mut().insert(client.clone());
    let budget = if tier == RequestTier::Health {
        Budget::Health
    } else {
        Budget::Admission
    };
    match limiters.check(budget, &RateKey::Client(client)) {
        Ok(()) => next.run(request).await,
        Err(retry_after_seconds) => {
            limited_response(&limiters, &request, retry_after_seconds, "admission", tier)
        }
    }
}

/// Apply the route-class budget after authentication and before activity I/O.
pub(crate) async fn enforce(
    State(limiters): State<Arc<RateLimiters>>,
    request: Request,
    next: Next,
) -> Response {
    if is_cors_preflight(&request) {
        return next.run(request).await;
    }
    let tier = request_tier(&request);
    if tier == RequestTier::Health {
        return next.run(request).await;
    }

    let client = request
        .extensions()
        .get::<ClientKey>()
        .cloned()
        .unwrap_or_else(|| resolve_client(&request, &limiters.trusted_proxies));
    let key = if uses_client_operation_key(request.uri().path()) {
        RateKey::Client(client.clone())
    } else {
        request
            .extensions()
            .get::<AuthenticatedSubject>()
            .map(|subject| RateKey::Subject(subject.subject().as_str().to_owned()))
            .unwrap_or(RateKey::Client(client))
    };
    let budget = match tier {
        RequestTier::Default => Budget::Default,
        RequestTier::Create => Budget::Create,
        RequestTier::Input => Budget::Input,
        RequestTier::Health => unreachable!("health is admitted above"),
    };
    match limiters.check(budget, &key) {
        Ok(()) => next.run(request).await,
        Err(retry_after_seconds) => {
            limited_response(&limiters, &request, retry_after_seconds, "operation", tier)
        }
    }
}

fn limited_response(
    limiters: &RateLimiters,
    request: &Request,
    retry_after_seconds: u64,
    stage: &'static str,
    tier: RequestTier,
) -> Response {
    limiters.note_rejection(stage, tier);
    let mut response = HttpProblem::RateLimited {
        retry_after_seconds,
    }
    .into_response();
    if is_rest_path(request.uri().path()) {
        response
            .headers_mut()
            .append(header::LINK, REST_LINKS.clone());
    }
    response
}

fn request_tier(request: &Request) -> RequestTier {
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or_else(|| request.uri().path());
    if matches!(request.method(), &Method::GET | &Method::HEAD)
        && matches!(path, "/livez" | "/ready")
    {
        return RequestTier::Health;
    }
    if request.method() != Method::POST {
        return RequestTier::Default;
    }
    if path == "/api/v1/sessions"
        || path == "/api/v1/agui/run"
        || is_session_operation(path, "plane")
        || is_session_operation(path, "revive")
        || is_launch_operation(path, "start")
    {
        return RequestTier::Create;
    }
    if is_session_operation(path, "input") || is_session_operation(path, "steer") {
        return RequestTier::Input;
    }
    RequestTier::Default
}

fn is_session_operation(path: &str, operation: &str) -> bool {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    matches!(
        (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
        ),
        (Some("api"), Some("v1"), Some("sessions"), Some(_), Some(found), None)
            if found == operation
    )
}

fn is_launch_operation(path: &str, operation: &str) -> bool {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    matches!(
        (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
        ),
        (Some("api"), Some("v1"), Some("launches"), Some(_), Some(found), None)
            if found == operation
    )
}

/// Routes that never require identity keep a network budget even when a
/// browser happens to attach a cookie. Otherwise the same public operation
/// changes buckets based on ambient credentials and can be used to exhaust a
/// subject's protected-route allowance.
fn uses_client_operation_key(path: &str) -> bool {
    path == "/openapi.json"
        || path == "/api/v1/conformance"
        || path == "/api/v1/agui/capabilities"
        || path == "/api/v1/mcp"
        || path == "/api/v1/a2a"
        || is_public_trigger_preview(path)
        || is_agui_trigger_preview(path)
        || path == "/.well-known/agent-card.json"
        || path == "/auth"
        || path.starts_with("/auth/")
}

fn is_rest_path(path: &str) -> bool {
    if path == "/openapi.json" {
        return true;
    }
    if path != "/api/v1" && path != "/api/v1/" && !path.starts_with("/api/v1/") {
        return false;
    }
    path != "/api/v1/agui"
        && !path.starts_with("/api/v1/agui/")
        && path != "/api/v1/mcp"
        && path != "/api/v1/a2a"
        && path
            .strip_prefix("/api/v1/sessions/")
            .and_then(|tail| tail.split_once('/'))
            .is_none_or(|(id, tail)| id.is_empty() || tail != "terminal")
}

fn is_public_trigger_preview(path: &str) -> bool {
    exact_path_segments(path, &["api", "v1", "triggers", "public", "*"])
}

fn is_agui_trigger_preview(path: &str) -> bool {
    exact_path_segments(path, &["api", "v1", "agui", "triggers", "*"])
}

fn exact_path_segments(path: &str, expected: &[&str]) -> bool {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    expected.iter().all(|expected| {
        segments
            .next()
            .is_some_and(|actual| *expected == "*" || actual == *expected)
    }) && segments.next().is_none()
}

fn resolve_client(request: &Request, trusted: &TrustedProxyConfig) -> ClientKey {
    let Some(peer) = request.extensions().get::<ConnectInfo<SocketAddr>>() else {
        return ClientKey::Unidentified;
    };
    let peer_ip = canonical_ip(peer.0.ip());
    let forwarded = trusted
        .cidrs
        .iter()
        .any(|network| network.contains(&peer_ip))
        .then(|| forwarded_client(request.headers(), trusted.forwarded_hops))
        .flatten();
    ClientKey::Network(normalize_client_ip(forwarded.unwrap_or(peer_ip)))
}

fn forwarded_client(headers: &HeaderMap, forwarded_hops: usize) -> Option<IpAddr> {
    let mut tokens = Vec::new();
    let mut found = false;
    for value in headers.get_all("x-forwarded-for") {
        found = true;
        let raw = value.to_str().ok()?;
        for token in raw.split(',') {
            let token = token.trim();
            if token.is_empty() {
                return None;
            }
            tokens.push(token);
        }
    }
    if !found {
        return None;
    }
    let offset = forwarded_hops.checked_add(1)?;
    let index = tokens.len().checked_sub(offset)?;
    tokens[index].parse::<IpAddr>().ok().map(canonical_ip)
}

fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(ip) => ip.to_ipv4_mapped().map_or(IpAddr::V6(ip), IpAddr::V4),
        ip => ip,
    }
}

fn normalize_client_ip(ip: IpAddr) -> IpAddr {
    match canonical_ip(ip) {
        IpAddr::V4(ip) => IpAddr::V4(ip),
        IpAddr::V6(ip) => {
            let prefix = u128::from(ip) & (u128::MAX << 64);
            IpAddr::V6(Ipv6Addr::from(prefix))
        }
    }
}

fn retry_after_seconds(wait: std::time::Duration) -> u64 {
    wait.as_secs()
        .saturating_add(u64::from(wait.subsec_nanos() != 0))
        .max(1)
}

#[cfg(test)]
fn generous_test_quota() -> Quota {
    Quota::per_second(std::num::NonZeroU32::new(1_000_000).unwrap())
}
