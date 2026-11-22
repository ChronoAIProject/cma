//! Retry explicit throttling of reads without replaying connection mutations.

use std::time::Duration;

use reqwest::{Method, RequestBuilder, Response, StatusCode};
use tokio::time::{timeout_at, Instant};

use super::{unavailable, ServiceConnectionError, PROVIDER_UNAVAILABLE};
use crate::auth::ProviderRetryAfter;

const ATTEMPTS: u8 = 2;
const MIN_RETRY_BUDGET: Duration = Duration::from_millis(750);

#[cfg(test)]
async fn send(request: RequestBuilder) -> Result<Response, ServiceConnectionError> {
    send_with_budget(request, super::PROVIDER_HTTP_TIMEOUT).await
}

#[cfg(test)]
async fn send_with_budget(
    request: RequestBuilder,
    budget: Duration,
) -> Result<Response, ServiceConnectionError> {
    send_until(request, Instant::now() + budget).await
}

pub(super) async fn send_until(
    request: RequestBuilder,
    deadline: Instant,
) -> Result<Response, ServiceConnectionError> {
    let (client, request) = request.build_split();
    let request = request.map_err(|_| unavailable(PROVIDER_UNAVAILABLE))?;
    // Creating a handoff has external effects. Neither an HTTP error nor a
    // transport failure proves it is safe to repeat that original operation.
    let endpoint = match request.url().path() {
        "/api/v1/user-services" => Some("user_services"),
        "/api/v1/assistant/readiness" => Some("readiness"),
        path if path
            .strip_prefix("/api/v1/keys/")
            .is_some_and(|id| !id.is_empty() && !id.contains('/')) =>
        {
            Some("credential")
        }
        _ => None,
    };
    if request.method() != Method::GET || endpoint.is_none() {
        return client
            .execute(request)
            .await
            .map_err(|_| unavailable(PROVIDER_UNAVAILABLE));
    }
    let endpoint = endpoint.expect("only requirement reads enter the retry loop");
    let stop = request
        .timeout()
        .and_then(|timeout| Instant::now().checked_add(*timeout))
        .map_or(deadline, |stop| stop.min(deadline));
    for attempt in 1..=ATTEMPTS {
        let remaining = stop.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(unavailable(PROVIDER_UNAVAILABLE));
        }
        let mut next = request
            .try_clone()
            .ok_or_else(|| unavailable(PROVIDER_UNAVAILABLE))?;
        // The retry shares the original budget, including consuming its body.
        // No task survives cancellation of the owning connection check.
        *next.timeout_mut() = Some(remaining);
        let response = timeout_at(stop, client.execute(next))
            .await
            .map_err(|_| unavailable(PROVIDER_UNAVAILABLE))?
            .map_err(|_| unavailable(PROVIDER_UNAVAILABLE))?;
        let status = response.status();
        if status != StatusCode::TOO_MANY_REQUESTS {
            if !status.is_success() {
                tracing::warn!(
                    endpoint,
                    status = status.as_u16(),
                    attempt,
                    "NyxID connection read failed"
                );
            }
            return Ok(response);
        }
        let jitter = u64::from(uuid::Uuid::new_v4().as_bytes()[0]) % 31;
        let delay = ProviderRetryAfter::from_headers(response.headers())
            .remaining()
            .saturating_add(Duration::from_millis(40 + jitter));
        let retry = attempt < ATTEMPTS
            && delay.saturating_add(MIN_RETRY_BUDGET)
                < stop.saturating_duration_since(Instant::now());
        tracing::warn!(
            endpoint,
            status = 429u16,
            attempt,
            retry,
            retry_after_ms = delay.as_millis().min(u128::from(u64::MAX)) as u64,
            "NyxID connection read throttled"
        );
        if !retry {
            return Ok(response);
        }
        // A 429 is not credential evidence; discard its body and repeat only
        // the exact GET. A long provider floor is never truncated to fit.
        drop(response);
        tokio::time::sleep(delay).await;
    }
    unreachable!("the final attempt always returns its response")
}

#[cfg(test)]
mod tests;
