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
        }
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
