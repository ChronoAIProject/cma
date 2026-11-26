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
//! only when that peer belongs to an explicitly configured GFE network. A
//! fixed right-anchored suffix identifies the upstream GFE actually observed.
//! If that upstream is a pinned Cloudflare edge, CMA advances one token only
//! when a single `CF-Connecting-IP` value agrees exactly and no Worker marker
//! is present. Ambiguity retains the observed upstream rather than trusting a
//! caller prefix. IPv4-mapped IPv6 is canonicalized to IPv4; ordinary IPv6
//! clients share a `/64`, so rotating privacy addresses does not mint fresh
//! budgets.
//!
//! Cheap pre-authentication guards protect each pod. Authenticated operation
//! budgets are shared through MongoDB, so rolling overlap retains one allowance.

use std::{
    net::{IpAddr, Ipv6Addr, SocketAddr},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use crate::{
    config::{RateLimitConfig, TrustedProxyConfig},
    is_cors_preflight, REST_LINKS,
};
use axum::{
    extract::{ConnectInfo, Request, State},
    http::{header, HeaderMap},
    middleware::Next,
    response::{IntoResponse, Response},
};
use governor::{clock::Clock, DefaultKeyedRateLimiter, Quota, RateLimiter};
use layer1_public::{rest::HttpProblem, AuthenticatedSubject};

mod anonymous;
mod lease;
mod shared;
mod tiers;
mod visitor;

use anonymous::{is_anonymous_public_route, AnonymousPublicBudget};
use tiers::{is_rest_path, request_tier, uses_client_operation_key, RequestTier};
#[allow(unused_imports)] // Consumed by the Trigger route composition that injects this slice.
pub(crate) use visitor::CmaLaunchQuota;
use visitor::VisitorLaunchQuota;

#[cfg(test)]
use axum::http::Method;

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

#[derive(Debug, Clone, Copy)]
enum Budget {
    Admission,
    Health,
    Default,
    Create,
    Input,
    Control,
}

/// Independent admission, health, and operation budgets.
pub(crate) struct RateLimiters {
    lifecycle: std::sync::OnceLock<Arc<dyn crate::control_admission::LifecycleAdmissionPolicy>>,
    shared: Option<(
        Arc<crate::store::request_limits::MongoRequestLimits>,
        RateLimitConfig,
    )>,
    admission: KeyedLimiter,
    health: KeyedLimiter,
    default: KeyedLimiter,
    create: KeyedLimiter,
    input: KeyedLimiter,
    control: KeyedLimiter,
    response_input_tier_enabled: bool,
    control_tier_enabled: bool,
    default_leases: lease::LeaseTable,
    anonymous_public: Option<AnonymousPublicBudget>,
    visitor_launches: VisitorLaunchQuota,
    trusted_proxies: TrustedProxyConfig,
    decisions: AtomicU64,
    rejections: AtomicU64,
}

impl RateLimiters {
    pub(crate) fn install_lifecycle(&self, config: crate::config::ControlAdmissionConfig) {
        let store = self
            .shared
            .as_ref()
            .expect("production limiter has a shared store")
            .0
            .clone();
        assert!(
            self.lifecycle
                .set(crate::control_admission::policy(config, store))
                .is_ok(),
            "lifecycle admission installed twice"
        );
    }

    pub(crate) fn lifecycle(
        &self,
    ) -> Option<Arc<dyn crate::control_admission::LifecycleAdmissionPolicy>> {
        self.lifecycle.get().cloned()
    }

    /// Build production budgets from validated configuration.
    pub(crate) fn from_config(
        config: RateLimitConfig,
        trusted_proxies: TrustedProxyConfig,
        shared: Arc<crate::store::request_limits::MongoRequestLimits>,
    ) -> Arc<Self> {
        let mut limiters = Self::from_quotas(
            Quota::per_minute(config.admission_per_minute),
            Quota::per_minute(config.health_per_minute),
            Quota::per_minute(config.default_per_minute),
            Quota::per_minute(config.create_per_minute),
            Quota::per_minute(config.input_per_minute),
            Quota::per_minute(config.control_per_minute),
            trusted_proxies,
        );
        limiters.anonymous_public = config.anonymous_public.enabled.then(|| {
            AnonymousPublicBudget::new(
                config.anonymous_public.per_minute,
                config.anonymous_public.concurrency,
            )
        });
        limiters.response_input_tier_enabled = config.response_input_tier_enabled;
        limiters.control_tier_enabled = config.control_tier_enabled;
        limiters.shared = Some((shared, config));
        Arc::new(limiters)
    }

    fn from_quotas(
        admission: Quota,
        health: Quota,
        default: Quota,
        create: Quota,
        input: Quota,
        control: Quota,
        trusted_proxies: TrustedProxyConfig,
    ) -> Self {
        Self {
            lifecycle: std::sync::OnceLock::new(),
            shared: None,
            admission: RateLimiter::keyed(admission),
            health: RateLimiter::keyed(health),
            default: RateLimiter::keyed(default),
            create: RateLimiter::keyed(create),
            input: RateLimiter::keyed(input),
            control: RateLimiter::keyed(control),
            response_input_tier_enabled: false,
            control_tier_enabled: false,
            default_leases: lease::LeaseTable::default(),
            anonymous_public: None,
            visitor_launches: VisitorLaunchQuota::default(),
            trusted_proxies,
            decisions: AtomicU64::new(0),
            rejections: AtomicU64::new(0),
        }
    }

    fn check(&self, budget: Budget, key: &RateKey) -> Result<(), u64> {
        let limiter = match budget {
            Budget::Admission => &self.admission,
            Budget::Health => &self.health,
            Budget::Default => &self.default,
            Budget::Create => &self.create,
            Budget::Input => &self.input,
            Budget::Control => &self.control,
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
            self.control.retain_recent();
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

    let tier = request_tier(
        &request,
        limiters.response_input_tier_enabled,
        limiters.control_tier_enabled,
    );
    let _anonymous_permit = if let Some(budget) = limiters
        .anonymous_public
        .as_ref()
        .filter(|_| is_anonymous_public_route(request.uri().path()))
    {
        match budget.try_admit() {
            Ok(permit) => Some(permit),
            Err(retry_after_seconds) => {
                return limited_response(
                    &limiters,
                    &request,
                    retry_after_seconds,
                    "anonymous_public",
                    tier,
                )
            }
        }
    } else {
        None
    };
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
    if request
        .extensions()
        .get::<crate::control_admission::Admitted>()
        .is_some()
    {
        return next.run(request).await;
    }
    if is_cors_preflight(&request)
        || request
            .extensions()
            .get::<crate::agent_manager::relay::ChargedQueuedCancellation>()
            .is_some()
    {
        return next.run(request).await;
    }
    let tier = request_tier(
        &request,
        limiters.response_input_tier_enabled,
        limiters.control_tier_enabled,
    );
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
        RequestTier::Control => Budget::Control,
        RequestTier::Health => unreachable!("health is admitted above"),
    };
    match limiters.check_operation(budget, &key).await {
        Ok(()) => next.run(request).await,
        Err(shared::Refusal::Limited(retry_after_seconds)) => {
            limited_response(&limiters, &request, retry_after_seconds, "operation", tier)
        }
        Err(shared::Refusal::Unavailable) => shared::unavailable_response(),
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

fn resolve_client(request: &Request, trusted: &TrustedProxyConfig) -> ClientKey {
    let Some(peer) = request.extensions().get::<ConnectInfo<SocketAddr>>() else {
        return ClientKey::Unidentified;
    };
    let peer_ip = canonical_ip(peer.0.ip());
    let forwarded = trusted
        .cidrs
        .iter()
        .any(|network| network.contains(&peer_ip))
        .then(|| forwarded_client(request.headers(), trusted))
        .flatten();
    ClientKey::Network(normalize_client_ip(forwarded.unwrap_or(peer_ip)))
}

fn forwarded_client(headers: &HeaderMap, trusted: &TrustedProxyConfig) -> Option<IpAddr> {
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
    let offset = trusted.forwarded_hops.checked_add(1)?;
    let index = tokens.len().checked_sub(offset)?;
    let upstream = tokens[index].parse::<IpAddr>().ok().map(canonical_ip)?;

    if !trusted
        .cloudflare_cidrs
        .iter()
        .any(|network| network.contains(&upstream))
    {
        return Some(upstream);
    }

    // Cloudflare Workers can alter the connecting-client value, while Pseudo
    // IPv4 overwrite mode carries the real address in CF-Connecting-IPv6. Keep
    // the authenticated edge as the key in either case instead of treating the
    // rewritten value as visitor authority.
    if headers.contains_key("cf-worker") || headers.contains_key("cf-connecting-ipv6") {
        return Some(upstream);
    }

    let visitor = index
        .checked_sub(1)
        .and_then(|index| tokens[index].parse::<IpAddr>().ok())
        .map(canonical_ip);
    let connecting = single_ip_header(headers, "cf-connecting-ip");
    Some(match (visitor, connecting) {
        (Some(visitor), Some(connecting))
            if visitor == connecting && !is_cloudflare_pseudo_ipv4(visitor) =>
        {
            visitor
        }
        _ => upstream,
    })
}

fn is_cloudflare_pseudo_ipv4(ip: IpAddr) -> bool {
    matches!(canonical_ip(ip), IpAddr::V4(ip) if ip.octets()[0] >= 240)
}

fn single_ip_header(headers: &HeaderMap, name: &'static str) -> Option<IpAddr> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    if value.is_empty() || value != value.trim() {
        return None;
    }
    value.parse::<IpAddr>().ok().map(canonical_ip)
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
#[path = "rate_limit_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "rate_limit_input_tests.rs"]
mod input_tests;

#[cfg(test)]
#[path = "rate_limit_control_tests.rs"]
mod control_tests;

#[cfg(test)]
#[path = "rate_limit/desktop_tests.rs"]
mod desktop_tests;

#[cfg(test)]
#[path = "rate_limit/anonymous_tests.rs"]
mod anonymous_tests;

#[cfg(test)]
#[path = "rate_limit/anonymous_policy_tests.rs"]
mod anonymous_policy_tests;

#[cfg(test)]
#[path = "rate_limit/anonymous_mongo_tests.rs"]
mod anonymous_mongo_tests;
