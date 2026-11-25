//! Distinguish an explicit credential refusal from unavailable provider evidence.

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(super) enum ProviderVerificationError {
    #[error("{0}")]
    Rejected(String),
    #[error("{0}")]
    Unavailable(String),
    /// Only these classified transport / gateway failures permit transient retry.
    #[error("identity provider unavailable: {0}")]
    Transient(&'static str),
    #[error("identity provider rate limited")]
    RateLimited(super::identity_throttle::RetryAfter),
}

/// Travels with an unfinished flight's result, so each consumer retains the
/// selected cause without borrowing the verification leader's request context.
#[derive(Debug, Clone)]
pub(super) struct VerificationFailure {
    pub error: ProviderVerificationError,
    pub cause: VerificationCause,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VerificationCause {
    Provider,
    Capacity,
    FenceCapacity,
    VerificationDeadline,
    CallerDeadline,
    PublicationEpochChanged,
    ReceiptExpired,
    ExpiryClockBounds,
    ConsumptionExpired,
    ConsumptionEpochChanged,
}

impl VerificationFailure {
    pub(super) fn new(error: ProviderVerificationError, cause: VerificationCause) -> Self {
        Self { error, cause }
    }

    /// Preserve known local capacity in the consumer's request context before
    /// the existing coarse provider projection records its fallback category.
    pub(super) fn record_local_cause(&self) {
        use layer2_core::dependency_diagnostics::{Dependency, Failure, Reason};
        if !matches!(self.error, ProviderVerificationError::Rejected(_))
            && matches!(
                self.cause,
                VerificationCause::Capacity | VerificationCause::FenceCapacity
            )
        {
            Failure::new(Dependency::Local, Reason::Capacity).record();
        }
    }

    pub(super) fn reason(&self) -> &'static str {
        match self.cause {
            VerificationCause::Provider => match &self.error {
                ProviderVerificationError::Rejected(_) => "rejected",
                ProviderVerificationError::Unavailable(_) => "unknown",
                ProviderVerificationError::RateLimited(_) => "rate_limited",
                ProviderVerificationError::Transient(reason) => match *reason {
                    "timeout" => "timeout",
                    "connection" => "connection",
                    "http_502" | "http_503" => "http_status",
                    "deadline" => "verification_deadline",
                    _ => "unknown",
                },
            },
            VerificationCause::Capacity => "verification_capacity",
            VerificationCause::FenceCapacity => "verification_fence_capacity",
            VerificationCause::VerificationDeadline => "verification_deadline",
            VerificationCause::CallerDeadline => "caller_deadline",
            VerificationCause::PublicationEpochChanged => "publication_epoch_changed",
            VerificationCause::ReceiptExpired => "receipt_expired",
            VerificationCause::ExpiryClockBounds => "expiry_clock_bounds",
            VerificationCause::ConsumptionExpired => "consumption_expired",
            VerificationCause::ConsumptionEpochChanged => "consumption_epoch_changed",
        }
    }

    pub(super) fn status(&self) -> Option<u16> {
        match &self.error {
            ProviderVerificationError::Transient("http_502") => Some(502),
            ProviderVerificationError::Transient("http_503") => Some(503),
            ProviderVerificationError::RateLimited(_) => Some(429),
            _ => None,
        }
    }
}

impl From<ProviderVerificationError> for VerificationFailure {
    fn from(error: ProviderVerificationError) -> Self {
        Self::new(error, VerificationCause::Provider)
    }
}

impl ProviderVerificationError {
    pub(super) fn reason(&self) -> &'static str {
        match self {
            Self::Transient(reason) => reason,
            Self::RateLimited(_) => "http_429",
            Self::Rejected(_) => "rejected",
            Self::Unavailable(_) => "invalid_evidence",
        }
    }

    /// Preserve the final typed verification reason before the public projection.
    /// This runs per caller, after single-flight fan-out.
    pub(super) fn record_unavailable(&self) {
        use layer2_core::dependency_diagnostics::{Dependency, Failure, Reason};
        let reason = match self.reason() {
            "timeout" => Reason::Timeout,
            "connection" => Reason::Connection,
            "http_502" => Reason::BadGateway,
            "http_503" => Reason::UpstreamUnavailable,
            "http_429" => Reason::RateLimited,
            "deadline" => Reason::VerificationDeadline,
            "invalid_evidence" => Reason::InvalidEvidence,
            _ => Reason::Unknown,
        };
        Failure::new(Dependency::IdentityProvider, reason).record();
        if let Self::RateLimited(hint) = self {
            hint.record();
        }
    }

    pub(super) fn transport(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            Self::Transient("timeout")
        } else if error.is_connect() || connection_io_failure(&error) {
            Self::Transient("connection")
        } else {
            Self::Unavailable("identity provider transport failed".into())
        }
    }

    pub(super) fn status(status: reqwest::StatusCode) -> Self {
        match status.as_u16() {
            401 | 403 => Self::Rejected("identity provider refused verification".into()),
            502 => Self::Transient("http_502"),
            503 => Self::Transient("http_503"),
            // The HTTP reader handles 429 with its Retry-After headers. Other
            // status codes cannot become transient configuration failures.
            _ => Self::Unavailable(format!(
                "identity provider returned HTTP {}",
                status.as_u16()
            )),
        }
    }

    pub(super) fn command_error(self) -> layer2_core::sandbox::agent_io::CommandAuthorizationError {
        use layer2_core::sandbox::agent_io::CommandAuthorizationError;
        match self {
            Self::Rejected(_) => CommandAuthorizationError::Revoked,
            Self::Unavailable(_) | Self::Transient(_) | Self::RateLimited(_) => {
                CommandAuthorizationError::Unavailable
            }
        }
    }
}

fn connection_io_failure(error: &reqwest::Error) -> bool {
    use std::error::Error;
    use std::io::ErrorKind;
    // is_request() also includes malformed HTTP responses. Only recognized
    // connection IO failures before headers can enter the transient class.
    let mut source = error.source();
    while let Some(cause) = source {
        if cause.downcast_ref::<std::io::Error>().is_some_and(|error| {
            matches!(
                error.kind(),
                ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
                    | ErrorKind::NotConnected
                    | ErrorKind::BrokenPipe
            )
        }) {
            return true;
        }
        source = cause.source();
    }
    false
}
