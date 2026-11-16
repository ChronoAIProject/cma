//! One retry of the actual credential's idempotent organization GET.
use std::time::Duration;

use layer2_core::resource_access::MembershipError;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use serde::de::DeserializeOwned;
use tokio::time::Instant;

use super::super::{identity_throttle::RetryAfter, AuthState};
use super::diagnostics::{Operation, Reason};

const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) struct AttemptFailure {
    error: MembershipError,
    transient: bool,
    retry_after: Option<RetryAfter>,
}

impl AttemptFailure {
    pub(super) fn terminal(error: MembershipError) -> Self {
        Self {
            error,
            transient: false,
            retry_after: None,
        }
    }

    pub(super) fn retry_transport(mut self, error: &reqwest::Error) -> Self {
        // Reuse the identity adapter's narrow IO classification. Malformed HTTP
        // and invalid evidence are not evidence of a recoverable connection.
        self.transient = error.is_timeout()
            || error.is_connect()
            || super::super::verification_error::connection_io_failure(error);
        self
    }

    pub(super) fn retry_timeout(mut self) -> Self {
        self.transient = true;
        self
    }

    pub(super) fn retry_status(mut self, status: u16, headers: &HeaderMap) -> Self {
        self.transient = matches!(status, 429 | 502 | 503 | 504);
        if self.transient && (status == 429 || headers.contains_key(RETRY_AFTER)) {
            self.retry_after = Some(RetryAfter::from_headers(headers));
        }
        self
    }
}

pub(super) fn transport_reason(error: &reqwest::Error) -> Reason {
    if error.is_timeout() {
        Reason::Timeout
    } else if error.is_connect() || super::super::verification_error::connection_io_failure(error) {
        Reason::Connection
    } else {
        Reason::Transport
    }
}

impl AuthState {
    pub(super) async fn organization_json<T: DeserializeOwned>(
        &self,
        endpoint: reqwest::Url,
        credential: &str,
        operation: Operation,
        deadline: Instant,
    ) -> Result<T, MembershipError> {
        for attempt in 0..2 {
            let stop = deadline.min(Instant::now() + ATTEMPT_TIMEOUT);
            let result = self
                .organization_attempt(endpoint.clone(), credential, operation, stop)
                .await;
            let failure = match result {
                Ok(value) => return Ok(value),
                Err(failure) => failure,
            };
            if attempt == 1 || !failure.transient {
                return Err(failure.error);
            }
            let jitter =
                Duration::from_millis(50 + u64::from(uuid::Uuid::new_v4().as_bytes()[0]) % 51);
            let delay = failure
                .retry_after
                .map(RetryAfter::remaining)
                .unwrap_or_default()
                .saturating_add(jitter);
            // Never shorten a provider delay, or begin a retry that cannot fit a
            // complete attempt within the original lookup's remaining budget.
            if delay.saturating_add(ATTEMPT_TIMEOUT)
                >= deadline.saturating_duration_since(Instant::now())
            {
                return Err(failure.error);
            }
            tokio::time::sleep(delay).await;
        }
        unreachable!("both bounded attempts return or retain a failure")
    }
}
