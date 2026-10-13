//! One gateway-wide limiter shared by `/console/login` and `/console/callback`.
//! Before sign-in a request has no identity to key on, so the limit is
//! global: anyone can delay console sign-in, but never ingress or tools, and
//! never push more than the configured token requests a minute to NyxID.
use std::num::NonZeroU32;

use axum::{http::StatusCode, response::Response};
use governor::{DefaultDirectRateLimiter, Quota, RateLimiter};

use super::{pages::signin, redirect::HOME};
use crate::telemetry::names;

pub(crate) const RATE_LIMITED: &str = "Too many sign-in attempts; try again in a minute.";

pub(crate) struct LoginLimiter(DefaultDirectRateLimiter);

impl LoginLimiter {
    pub(crate) fn new(per_minute: u32) -> Self {
        let rate = NonZeroU32::new(per_minute).unwrap_or(NonZeroU32::MIN);
        Self(RateLimiter::direct(Quota::per_minute(rate)))
    }

    /// Admit one sign-in step (`None`), or answer the same-origin sign-in
    /// page with 429.
    pub(crate) fn refusal(&self) -> Option<Response> {
        if self.0.check().is_ok() {
            return None;
        }
        metrics::counter!(names::CONSOLE_LOGINS, names::OUTCOME => "rate_limited").increment(1);
        tracing::warn!(outcome = "rate_limited", "console login");
        Some(signin::sign_in(
            StatusCode::TOO_MANY_REQUESTS,
            HOME,
            Some(RATE_LIMITED),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_limit_answers_the_429_sign_in_page() {
        let limiter = LoginLimiter::new(2);
        assert!(limiter.refusal().is_none());
        assert!(limiter.refusal().is_none());
        let refused = limiter.refusal().unwrap();
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        let body = axum::body::to_bytes(refused.into_body(), 1 << 20)
            .await
            .unwrap();
        let page = String::from_utf8(body.to_vec()).unwrap();
        assert!(page.contains(RATE_LIMITED));
        assert!(page.contains("href=\"/console/login?return_to=%2Fconsole\""));
    }
}
