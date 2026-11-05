use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Provider image request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderImage {
    pub uri: String,
}

/// OpenSandbox creation request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSandboxRequest {
    /// Internal prepaid authority; never part of an immutable provider request digest.
    #[serde(skip)]
    pub funded_until: Option<u64>,
    pub image: ProviderImage,
    pub entrypoint: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub resource_limits: BTreeMap<String, String>,
    /// An absent timeout serializes as null; the provider distinguishes it from omission.
    pub timeout: Option<u64>,
    pub metadata: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<super::ProviderVolume>,
}

impl ProviderSandboxRequest {
    pub(crate) fn validate_volumes(&self) -> Result<(), ProviderError> {
        super::volumes::validate(&self.volumes)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSandboxStatus {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub reason: Option<String>,
}

/// Forward-compatible provider lifecycle row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSandbox {
    pub id: String,
    #[serde(default)]
    pub status: Option<ProviderSandboxStatus>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

impl ProviderSandbox {
    pub fn is_running(&self) -> bool {
        self.status
            .as_ref()
            .is_some_and(|status| status.state.eq_ignore_ascii_case("running"))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pagination {
    #[serde(default)]
    pub page: u32,
    #[serde(default)]
    pub page_size: u32,
    #[serde(default)]
    pub total_items: u64,
    #[serde(default)]
    pub total_pages: u32,
    #[serde(default)]
    pub has_next_page: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProviderEndpoint {
    pub endpoint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    #[error("sandbox provider unreachable: {0}")]
    Unreachable(String),
    #[error("sandbox provider returned {status}: {detail}")]
    Rejected { status: u16, detail: String },
    #[error("sandbox provider response not usable: {0}")]
    Malformed(String),
    #[error("sandbox listing did not end after {pages} pages of {page_size}")]
    Incomplete { pages: u32, page_size: u32 },
    #[error("sandbox provider identity changed: {0}")]
    IdentityMismatch(String),
    #[error("sandbox provider does not support this operation: {0}")]
    Unsupported(String),
}

impl From<ProviderError> for crate::AdapterError {
    fn from(error: ProviderError) -> Self {
        match error {
            ProviderError::Unreachable(detail) => crate::AdapterError::Unavailable(detail),
            ProviderError::Incomplete { pages, page_size } => crate::AdapterError::Unavailable(
                format!("sandbox listing did not end after {pages} pages of {page_size}"),
            ),
            ProviderError::Rejected {
                status: 429,
                detail,
            } => crate::AdapterError::AtCapacity(detail),
            ProviderError::Rejected {
                status: 404,
                detail,
            } => crate::AdapterError::Unavailable(detail),
            ProviderError::Rejected { status, detail } => {
                crate::AdapterError::Rejected(format!("{status}: {detail}"))
            }
            ProviderError::Malformed(detail) | ProviderError::IdentityMismatch(detail) => {
                crate::AdapterError::Rejected(detail)
            }
            ProviderError::Unsupported(detail) => crate::AdapterError::Unsupported(detail),
        }
    }
}
