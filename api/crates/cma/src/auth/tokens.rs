//! Identity-provider discovery document fields used by CMA.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub struct ProviderMetadata {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(default)]
    pub userinfo_endpoint: Option<String>,
    #[serde(default)]
    pub introspection_endpoint: Option<String>,
}
