//! Per-pod protection for anonymous public Trigger reads.

use std::{num::NonZeroU32, sync::Arc};

use governor::{clock::Clock, DefaultDirectRateLimiter, Quota, RateLimiter};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::retry_after_seconds;

const DEFAULT_PER_MINUTE: NonZeroU32 = NonZeroU32::new(1_200).expect("non-zero default");
const DEFAULT_CONCURRENCY: NonZeroU32 = NonZeroU32::new(16).expect("non-zero default");

/// A process-local total budget shared by every anonymous public Trigger route.
pub(super) struct AnonymousPublicBudget {
    rate: DefaultDirectRateLimiter,
    inflight: Arc<Semaphore>,
}

impl AnonymousPublicBudget {
    pub(super) fn new(per_minute: NonZeroU32, concurrency: NonZeroU32) -> Self {
        Self {
            rate: RateLimiter::direct(Quota::per_minute(per_minute)),
            inflight: Arc::new(Semaphore::new(concurrency.get() as usize)),
        }
    }

    pub(super) fn try_admit(&self) -> Result<OwnedSemaphorePermit, u64> {
        let permit = self
            .inflight
            .clone()
            .try_acquire_owned()
            .map_err(|_| 1_u64)?;
        match self.rate.check() {
            Ok(()) => Ok(permit),
            Err(refusal) => Err(retry_after_seconds(
                refusal.wait_time_from(self.rate.clock().now()),
            )),
        }
    }
}

impl Default for AnonymousPublicBudget {
    fn default() -> Self {
        Self::new(DEFAULT_PER_MINUTE, DEFAULT_CONCURRENCY)
    }
}

pub(super) fn is_anonymous_public_route(path: &str) -> bool {
    is_public_trigger_preview(path)
        || exact_path_segments(path, &["api", "v1", "triggers", "public", "*", "open", "*"])
        || exact_path_segments(
            path,
            &["api", "v1", "triggers", "public", "*", "entry-intent"],
        )
}

pub(super) fn is_public_trigger_preview(path: &str) -> bool {
    exact_path_segments(path, &["api", "v1", "triggers", "public", "*"])
        || exact_path_segments(path, &["api", "v1", "triggers", "public", "*", "badge.svg"])
}

/// Keep identity-free routes on a network budget even when a browser sends a cookie.
pub(super) fn uses_client_operation_key(path: &str) -> bool {
    if matches!(
        path,
        "/auth/connections/check" | "/auth/connections/start" | "/auth/connections/cancel"
    ) {
        return false;
    }
    path == "/openapi.json"
        || path == "/api/v1/conformance"
        || is_public_trigger_preview(path)
        || path == "/.well-known/agent-card.json"
        || path == "/auth"
        || path.starts_with("/auth/")
}

fn exact_path_segments(path: &str, expected: &[&str]) -> bool {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    expected.iter().all(|expected| {
        segments
            .next()
            .is_some_and(|actual| *expected == "*" || actual == *expected)
    }) && segments.next().is_none()
}
