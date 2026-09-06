//! Public SDK failures.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

/// A result returned by this SDK.
pub type Result<T> = std::result::Result<T, Error>;

/// Every failure returned by the CMA client.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The service returned a structured non-success response.
    Api {
        /// HTTP response status.
        status: u16,
        /// Stable machine-readable code, when supplied.
        code: Option<String>,
        /// Human-readable problem detail.
        message: String,
        /// Usable response correlation id, when supplied.
        request_id: Option<String>,
        /// Positive `Retry-After` delay supplied as seconds or an HTTP date.
        retry_after: Option<Duration>,
        /// Structured problem extensions, preserved without interpreting prose.
        extensions: BTreeMap<String, serde_json::Value>,
    },
    /// The request could not obtain an HTTP answer.
    Transport {
        /// Secret-free transport failure detail.
        message: String,
    },
    /// The peer's answer violated the expected wire boundary.
    Protocol {
        /// Description of the invalid boundary.
        message: String,
    },
    /// Caller input cannot be represented safely on the wire.
    Validation {
        /// Description of the invalid request value.
        message: String,
    },
}

impl Error {
    pub(crate) fn validation(message: impl Into<String>) -> Self {
        Self::Validation {
            message: message.into(),
        }
    }

    /// Whether a failed request might succeed later; the SDK never retries mutations.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Transport { .. }
                | Self::Api {
                    status: 429 | 500..=599,
                    ..
                }
        )
    }

    /// Whether the service requires a fresh authentication credential.
    pub fn is_unauthenticated(&self) -> bool {
        matches!(self, Self::Api { status: 401, .. })
    }

    /// A string-valued problem extension such as `field`, `path`, or `reason`.
    pub fn extension(&self, name: &str) -> Option<&str> {
        match self {
            Self::Api { extensions, .. } => extensions.get(name)?.as_str(),
            _ => None,
        }
    }
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
            Self::Api {
                status,
                code,
                message,
                ..
            } => {
                if let Some(code) = code {
                    write!(f, "CMA API error {status} ({code}): {message}")
                } else {
                    write!(f, "CMA API error {status}: {message}")
                }
            }
            Self::Transport { message } => write!(f, "CMA transport error: {message}"),
            Self::Protocol { message } => write!(f, "CMA protocol error: {message}"),
            Self::Validation { message } => write!(f, "CMA validation error: {message}"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_has_a_useful_display_message() {
        let errors = [
            Error::Api {
                status: 503,
                code: Some("rebuilding".into()),
                message: "not ready".into(),
                request_id: Some("request-801".into()),
                retry_after: None,
                extensions: BTreeMap::new(),
            },
            Error::Api {
                status: 404,
                code: None,
                message: "not found".into(),
                request_id: None,
                retry_after: None,
                extensions: BTreeMap::new(),
            },
            Error::transport("connection closed"),
            Error::protocol("bad document"),
        ];
        for error in errors {
            assert!(!error.to_string().is_empty());
            let _: &dyn std::error::Error = &error;
        }
    }
}
