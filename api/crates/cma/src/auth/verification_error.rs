//! Distinguish an explicit credential refusal from unavailable provider evidence.

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(super) enum ProviderVerificationError {
    #[error("{0}")]
    Rejected(String),
    #[error("{0}")]
    Unavailable(String),
    /// Only these classified transport / gateway failures permit retry and fallback.
    #[error("identity provider unavailable: {0}")]
    Transient(&'static str),
}

impl ProviderVerificationError {
    pub(super) fn reason(&self) -> &'static str {
        match self {
            Self::Transient(reason) => reason,
            Self::Rejected(_) => "rejected",
            Self::Unavailable(_) => "invalid_evidence",
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
            // In particular, never bypass 429 limits or retry configuration errors.
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
            Self::Unavailable(_) | Self::Transient(_) => CommandAuthorizationError::Unavailable,
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
