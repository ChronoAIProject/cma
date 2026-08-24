//! Public SDK failures.

use std::fmt;

/// A result returned by this SDK.
pub type Result<T> = std::result::Result<T, Error>;

/// Every failure a CMA client can act on.
///
/// Capability refusals are permanent for the current session. API failures are
/// structured service responses. Transport failures may succeed when retried,
/// while protocol failures mean the received boundary did not satisfy the wire
/// contract and are never silently discarded.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The session was never granted a required capability.
    CapabilityRefused {
        /// The server's machine-readable capability name.
        capability: String,
    },
    /// The service refused a request with a machine-readable problem document.
    Api {
        /// HTTP response status.
        status: u16,
        /// Stable machine-readable problem code.
        code: String,
        /// Human-readable problem detail.
        message: String,
    },
    /// The request could not cross the HTTP boundary.
    Transport {
        /// A secret-free description of the transport failure.
        message: String,
    },
    /// The peer's response did not satisfy the CMA wire contract.
    Protocol {
        /// A description of the violated boundary rule.
        message: String,
    },
}

impl Error {
    pub(crate) fn transport(message: impl Into<String>) -> Self {
        Self::Transport {
            message: message.into(),
        }
    }

    pub(crate) fn protocol(message: impl Into<String>) -> Self {
        Self::Protocol {
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CapabilityRefused { capability } => {
                write!(f, "session capability refused: {capability}")
            }
            Self::Api {
                status,
                code,
                message,
            } => write!(f, "CMA API error {status} ({code}): {message}"),
            Self::Transport { message } => write!(f, "CMA transport error: {message}"),
            Self::Protocol { message } => write!(f, "CMA protocol error: {message}"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_public_variant_has_a_useful_display_message() {
        let errors = [
            Error::CapabilityRefused {
                capability: "steer".into(),
            },
            Error::Api {
                status: 409,
                code: "invalid_state".into(),
                message: "session is rotating".into(),
            },
            Error::transport("connection closed"),
            Error::protocol("bad frame"),
        ];
        for error in errors {
            assert!(!error.to_string().is_empty());
            let _: &dyn std::error::Error = &error;
        }
    }
}
