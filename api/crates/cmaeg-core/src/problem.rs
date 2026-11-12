//! RFC 9457 problem documents for CMAEG refusals.

use std::fmt;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProblemCode {
    InvalidRequest,
    NotAuthenticated,
    Forbidden,
    ChannelNotFound,
    ChannelRestoreUnavailable,
    ChannelInvalid,
    ProviderUnavailable,
    ProviderRejected,
    ProviderBusy,
    Draining,
    EventRefInvalid,
    EventRefExpired,
    CallbackInvalid,
    AdmissionRefused,
    RateLimited,
    Conflict,
    PayloadTooLarge,
    UnsupportedMediaType,
    Internal,
    GithubSignatureInvalid,
    GithubInstallationForbidden,
    GithubResourceNotFound,
    GithubDeliveryConflict,
    GithubSourceUnavailable,
    GithubDispatchUnavailable,
    GithubIntegrationRetired,
    GithubProofWithdrawn,
    GithubDeliveryExpired,
    /// The event exists but has no attachment at the requested index.
    AttachmentNotFound,
    /// Inbound media is disabled, or a media read's event_ref sealed no
    /// activity digest to verify the activity against.
    MediaUnavailable,
}

/// Bounded metadata about a provider refusal. This is safe to expose to a
/// gateway peer: it contains no provider response body, credentials or
/// request data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpstreamRefusal {
    pub operation: String,
    pub status: u16,
    pub code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Problem {
    /// HTTP-only retry guidance, never serialized into the problem body.
    #[serde(skip)]
    #[schema(ignore)]
    pub retry_after_seconds: Option<u64>,
    #[serde(rename = "type", default = "default_type")]
    pub r#type: String,
    pub title: String,
    pub status: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    pub code: ProblemCode,
    /// Optional provider metadata for relay consumers. Older responses omit
    /// this field and retain the existing RFC 9457 shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<UpstreamRefusal>,
}

fn default_type() -> String {
    "about:blank".to_owned()
}

impl Problem {
    /// Construct a problem with the standard RFC 9457 type.
    pub fn new(code: ProblemCode, status: u16, detail: impl Into<String>) -> Self {
        Self {
            retry_after_seconds: None,
            r#type: default_type(),
            title: code.to_string().replace('_', " "),
            status,
            detail: Some(detail.into()),
            instance: None,
            code,
            upstream: None,
        }
    }

    pub fn with_upstream(mut self, upstream: UpstreamRefusal) -> Self {
        self.upstream = Some(upstream);
        self
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({})", self.code, self.status)?;
        if let Some(detail) = &self.detail {
            write!(formatter, ": {detail}")?;
        }
        Ok(())
    }
}

impl fmt::Display for ProblemCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = serde_json::to_string(self).expect("problem code is a string enum");
        formatter.write_str(value.trim_matches('"'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_guidance_is_http_only_and_does_not_change_retained_problem_json() {
        let original = Problem::new(ProblemCode::ProviderUnavailable, 503, "context_unavailable");
        let bytes = serde_json::to_vec(&original).unwrap();
        let mut guided = original.clone();
        guided.retry_after_seconds = Some(10);
        assert_eq!(serde_json::to_vec(&guided).unwrap(), bytes);
        assert_eq!(serde_json::from_slice::<Problem>(&bytes).unwrap(), original);
    }
}
