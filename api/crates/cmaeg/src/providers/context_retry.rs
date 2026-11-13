//! Bounded read-only recovery; a failed lookup never proves absent admission.
use super::{EventContextSnapshot, ProviderError};
use ring::rand::{SecureRandom, SystemRandom};
use std::{future::Future, time::Duration};
use tokio::time::Instant;

pub(super) const ATTEMPT_BOUND: Duration = Duration::from_secs(3);
const TOTAL_BOUND: Duration = Duration::from_secs(8);
const MIN_BACKOFF: Duration = Duration::from_millis(500);

pub(crate) struct ContextFailure {
    pub error: ProviderError,
    pub retry_after: Option<Duration>,
    pub class: &'static str,
    pub status: Option<u16>,
}

impl From<ProviderError> for ContextFailure {
    fn from(error: ProviderError) -> Self {
        let (class, status) = match &error {
            ProviderError::Rejected { status, .. } => ("authority_refusal", Some(*status)),
            ProviderError::ResponseMalformed(_) => ("malformed", None),
            ProviderError::InvalidRequest(_) => ("invalid", None),
            ProviderError::UnknownProvider(_) => ("configuration", None),
            _ => ("credential_tracking", None),
        };
        Self {
            error,
            retry_after: None,
            class,
            status,
        }
    }
}

impl ContextFailure {
    pub fn transport(error: reqwest::Error) -> Self {
        Self::transient(if error.is_timeout() {
            "timeout"
        } else {
            "transport"
        })
    }

    fn transient(class: &'static str) -> Self {
        Self {
            error: ProviderError::Unavailable("provider context is unavailable".into()),
            retry_after: Some(MIN_BACKOFF),
            class,
            status: None,
        }
    }

    pub fn response(status: u16, retry_after: Option<&reqwest::header::HeaderValue>) -> Self {
        let transient = status == 429 || (500..=599).contains(&status);
        // An unparseable minimum cannot authorize an earlier automatic retry.
        let minimum = match retry_after {
            None => Some(MIN_BACKOFF),
            Some(value) => value
                .to_str()
                .ok()
                .and_then(|v| {
                    v.parse::<u64>().ok().map(Duration::from_secs).or_else(|| {
                        httpdate::parse_http_date(v).ok().map(|date| {
                            date.duration_since(std::time::SystemTime::now())
                                .unwrap_or_default()
                        })
                    })
                })
                .map(|v| v.max(MIN_BACKOFF)),
        };
        Self {
            error: ProviderError::Unavailable("provider context is unavailable".into()),
            retry_after: transient.then_some(minimum).flatten(),
            class: if transient {
                "upstream_transient"
            } else {
                "upstream_refusal"
            },
            status: Some(status),
        }
    }
}

/// At most two sequential GETs, each at most three seconds, including body read.
/// Retry sleep belongs to this future: cancellation leaves no detached work.
pub(super) async fn retry<F, Fut>(
    mut read: F,
) -> Result<Option<EventContextSnapshot>, ContextFailure>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Option<EventContextSnapshot>, ContextFailure>>,
{
    let deadline = Instant::now() + TOTAL_BOUND;
    for attempt in 1..=2 {
        let started = Instant::now();
        let result = tokio::time::timeout_at((started + ATTEMPT_BOUND).min(deadline), read())
            .await
            .unwrap_or_else(|_| Err(ContextFailure::transient("timeout")));
        match result {
            Ok(value) => {
                tracing::info!(
                    attempt,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    outcome = if value.is_some() {
                        "retained"
                    } else {
                        "absent"
                    },
                    "provider context lookup"
                );
                return Ok(value);
            }
            Err(failure) => {
                tracing::warn!(
                    attempt,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    error_class = failure.class,
                    upstream_status = failure.status,
                    "provider context lookup failed"
                );
                let Some(minimum) = failure.retry_after.filter(|_| attempt < 2) else {
                    return Err(failure);
                };
                let delay = minimum.saturating_add(jitter());
                // Do not shorten Retry-After to squeeze a probe into the budget.
                if delay.saturating_add(ATTEMPT_BOUND)
                    > deadline.saturating_duration_since(Instant::now())
                {
                    return Err(failure);
                }
                tokio::time::sleep(delay).await;
            }
        }
    }
    unreachable!("the final attempt always returns")
}

fn jitter() -> Duration {
    let mut bytes = [0; 8];
    if SystemRandom::new().fill(&mut bytes).is_err() {
        return MIN_BACKOFF;
    }
    Duration::from_millis(u64::from_le_bytes(bytes) % 501)
}

#[cfg(test)]
#[path = "context_retry_tests.rs"]
mod tests;
