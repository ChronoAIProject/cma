//! Bounded retrieval of inbound media: the sources behind each attachment
//! locator, byte-capped bodies, pod-wide and per-channel budgets,
//! deadline-bounded response bodies, content sniffing, a byte-bounded
//! derived-media cache behind single-flight jobs, and the bounded transforms
//! that turn fetched bytes into what an agent can consume.
//!
//! Nothing here runs unless a later caller composes it, and every caller is
//! gated on `CMAEG_MEDIA_ENABLED`. Errors are classifications only: no URL,
//! header or upstream body can travel in a [`FetchError`].
pub mod body;
pub mod budget;
pub mod cache;
pub mod forward;
pub mod image;
pub mod jobs;
pub mod sniff;
pub mod source;
pub mod text;

use crate::MediaConfig;
use std::fmt;

/// The media budgets, as configured.
///
/// Every field is public so the budget types can be built from it before any
/// caller reads the remaining fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaLimits {
    pub max_bytes: u64,
    pub inflight_bytes: u64,
    pub response_bytes: u64,
    pub max_jobs: u32,
    pub cache_bytes: u64,
    pub fetch_per_minute: u32,
    pub download_bytes_per_hour: u64,
}

impl MediaLimits {
    pub fn from_config(config: &MediaConfig) -> Self {
        Self {
            max_bytes: config.max_bytes,
            inflight_bytes: config.inflight_bytes,
            response_bytes: config.response_bytes,
            max_jobs: config.max_jobs,
            cache_bytes: config.cache_bytes,
            fetch_per_minute: config.rate_per_minute,
            download_bytes_per_hour: config.download_bytes_per_hour,
        }
    }
}

/// Why an attachment's bytes could not be produced.
///
/// The variants carry no data on purpose: whatever reaches a log line, a span
/// or a client is one of these fixed codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchError {
    /// No locator, or a locator this activity and channel cannot resolve.
    NotRetrievable,
    TooLarge,
    Unauthorized,
    NotFound,
    TimedOut,
    /// Any 5xx other than 503, or a transport failure.
    Upstream,
    /// Refused by policy or by the upstream for a reason other than the above.
    Rejected,
    /// The upstream answered 503: a transient overload worth retrying later.
    Busy,
    RateLimited,
}

impl FetchError {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotRetrievable => "not_retrievable",
            Self::TooLarge => "too_large",
            Self::Unauthorized => "unauthorized",
            Self::NotFound => "not_found",
            Self::TimedOut => "timed_out",
            Self::Upstream => "upstream",
            Self::Rejected => "rejected",
            Self::Busy => "busy",
            Self::RateLimited => "rate_limited",
        }
    }
}

impl fmt::Display for FetchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::error::Error for FetchError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_follow_the_configuration() {
        let limits = MediaLimits::from_config(&MediaConfig::default());
        assert_eq!(
            limits,
            MediaLimits {
                max_bytes: 20_971_520,
                inflight_bytes: 67_108_864,
                response_bytes: 25_165_824,
                max_jobs: 6,
                cache_bytes: 33_554_432,
                fetch_per_minute: 60,
                download_bytes_per_hour: 268_435_456,
            }
        );
    }

    #[test]
    fn every_error_has_a_stable_code() {
        let codes: Vec<_> = [
            FetchError::NotRetrievable,
            FetchError::TooLarge,
            FetchError::Unauthorized,
            FetchError::NotFound,
            FetchError::TimedOut,
            FetchError::Upstream,
            FetchError::Rejected,
            FetchError::Busy,
            FetchError::RateLimited,
        ]
        .iter()
        .map(|error| (error.as_str(), error.to_string()))
        .collect();
        assert_eq!(
            codes.iter().map(|(code, _)| *code).collect::<Vec<_>>(),
            [
                "not_retrievable",
                "too_large",
                "unauthorized",
                "not_found",
                "timed_out",
                "upstream",
                "rejected",
                "busy",
                "rate_limited"
            ]
        );
        assert!(codes.iter().all(|(code, shown)| code == shown));
    }
}
