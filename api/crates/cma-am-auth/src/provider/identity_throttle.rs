//! Provider backoff is a lower bound, independent of the caller's wait budget.
use std::{cell::Cell, future::Future, time::Duration};

use reqwest::header::{HeaderMap, RETRY_AFTER};
use tokio::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryAfter {
    observed: Instant,
    delay: Duration,
}

impl RetryAfter {
    pub fn from_headers(headers: &HeaderMap) -> Self {
        // Missing/invalid hints must not create a hot retry loop. Multiple valid
        // hints are conservatively combined, never shortened to fit our budget.
        let delay = headers
            .get_all(RETRY_AFTER)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter_map(|value| parse(value, std::time::SystemTime::now()))
            .max()
            .unwrap_or(Duration::from_secs(1));
        Self {
            observed: Instant::now(),
            delay,
        }
    }

    pub fn remaining(self) -> Duration {
        self.delay.saturating_sub(self.observed.elapsed())
    }

    pub fn seconds(self) -> u64 {
        let remaining = self.remaining();
        remaining
            .as_secs()
            .saturating_add(u64::from(remaining.subsec_nanos() > 0))
            .max(1)
    }

    pub fn record(self) {
        let _ = HINT.try_with(|hint| hint.set(Some(hint.get().unwrap_or(0).max(self.seconds()))));
    }
}

fn parse(value: &str, now: std::time::SystemTime) -> Option<Duration> {
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return value.parse::<u64>().ok().map(Duration::from_secs);
    }
    httpdate::parse_http_date(value)
        .ok()
        .map(|date| date.duration_since(now).unwrap_or_default())
}

tokio::task_local! { static HINT: Cell<Option<u64>>; }

pub async fn capture<F: Future>(future: F) -> (F::Output, Option<u64>) {
    HINT.scope(Cell::new(None), async {
        let result = future.await;
        (result, HINT.with(Cell::get))
    })
    .await
}

/// Concurrent endpoint rejection is handled before this selection. If either
/// endpoint throttles, retrying both must respect the longer provider delay.
pub fn select<T, U>(
    first: &Result<T, super::verification_error::ProviderVerificationError>,
    second: &Result<U, super::verification_error::ProviderVerificationError>,
) -> Option<super::verification_error::ProviderVerificationError> {
    use super::verification_error::ProviderVerificationError as Error;
    [first.as_ref().err(), second.as_ref().err()]
        .into_iter()
        .flatten()
        .filter_map(|error| match error {
            Error::RateLimited(hint) => Some(*hint),
            _ => None,
        })
        .max_by_key(|hint| hint.remaining())
        .map(Error::RateLimited)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_parsing_never_truncates_a_provider_delay() {
        let now = std::time::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        assert_eq!(parse("12", now), Some(Duration::from_secs(12)));
        assert_eq!(parse("0", now), Some(Duration::ZERO));
        assert_eq!(
            parse(&u64::MAX.to_string(), now),
            Some(Duration::from_secs(u64::MAX))
        );
        assert_eq!(
            parse(&httpdate::fmt_http_date(now + Duration::from_secs(30)), now),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            parse(&httpdate::fmt_http_date(now - Duration::from_secs(30)), now),
            Some(Duration::ZERO)
        );
        for value in ["", "-1", "1.5", "+1", "NaN", "18446744073709551616"] {
            assert_eq!(parse(value, now), None);
        }
        let mut headers = HeaderMap::new();
        assert_eq!(RetryAfter::from_headers(&headers).seconds(), 1);
        headers.append(RETRY_AFTER, "3".parse().unwrap());
        headers.append(RETRY_AFTER, "60".parse().unwrap());
        assert_eq!(RetryAfter::from_headers(&headers).seconds(), 60);
    }
}
