//! Public SDK failures.

use std::fmt;

/// A result returned by this SDK.
pub type Result<T> = std::result::Result<T, Error>;

/// Every failure returned by the temporary CMA client.
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
            },
            Error::Api {
                status: 404,
                code: None,
                message: "not found".into(),
                request_id: None,
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
