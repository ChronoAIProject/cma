//! Default-off HTTP lifecycle load containment. Deferred worker demand is separate.
use std::{sync::Arc, time::Instant};

use crate::config::ControlAdmissionConfig;

mod budget;
mod http;
mod policy;
pub(crate) use budget::{ResourceBudget, StagePermit};
pub(crate) use http::{enforce, Admitted};
pub(crate) use policy::{LifecycleAdmissionPolicy, Policy};

#[cfg(test)]
mod http_tests;
#[cfg(test)]
mod mongo_tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Class {
    Stop,
    Delete,
}

impl Class {
    fn index(self) -> usize {
        match self {
            Self::Stop => 0,
            Self::Delete => 1,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Delete => "delete",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    Caller(u64),
    Capacity,
    Unavailable,
}

pub(crate) trait AdmissionClock: Send + Sync {
    fn now(&self) -> Instant;
}

struct SystemClock;
impl AdmissionClock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

pub(crate) fn policy(
    config: ControlAdmissionConfig,
    store: Arc<crate::store::request_limits::MongoRequestLimits>,
) -> Arc<dyn LifecycleAdmissionPolicy> {
    Arc::new(Policy::new(config, store, Arc::new(SystemClock)))
}

/// Share one process budget with trusted in-process delegated dispatch. Public
/// requests already charged before activity I/O carry the private admission mark.
pub(crate) fn routes(
    router: axum::Router,
    policy: Option<Arc<dyn LifecycleAdmissionPolicy>>,
) -> axum::Router {
    match policy {
        Some(policy) => router.layer(axum::middleware::from_fn_with_state(policy, enforce)),
        None => router,
    }
}

/// Use registered route metadata; an arbitrary suffix never selects a quota.
pub(crate) fn classify(request: &axum::extract::Request) -> Option<(Class, String)> {
    use axum::{extract::MatchedPath, http::Method};
    let matched = request.extensions().get::<MatchedPath>()?.as_str();
    let class = match (request.method(), matched) {
        (&Method::POST, "/api/v1/agents/{id}/stop") => Class::Stop,
        (&Method::DELETE, "/api/v1/agents/{id}") => Class::Delete,
        _ => return None,
    };
    let path = request.uri().path().strip_prefix("/api/v1/agents/")?;
    let id = if class == Class::Stop {
        path.strip_suffix("/stop")?
    } else {
        path
    };
    if id.is_empty() || id.len() > 256 || id.contains('/') {
        return None;
    }
    Some((class, id.to_owned()))
}

/// The production request limiters with lifecycle containment installed.
pub(crate) fn limiters(
    config: &crate::config::Config,
    shared: std::sync::Arc<crate::store::request_limits::MongoRequestLimits>,
) -> std::sync::Arc<crate::rate_limit::RateLimiters> {
    let limits = crate::rate_limit::RateLimiters::from_config(
        config.rate_limits,
        config.trusted_proxies.clone(),
        shared,
    );
    limits.install_lifecycle(config.control_admission);
    limits
}
