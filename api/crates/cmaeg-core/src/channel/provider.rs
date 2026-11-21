//! Retained provider binding identity and target validation.
use crate::{
    ids::{BindingId, ProviderSlug},
    require_string, ValidationError,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConformanceLevel {
    L1,
    L2,
    L3,
    L4,
    L5,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderBinding {
    pub slug: ProviderSlug,
    #[serde(default, skip_serializing_if = "ProviderTarget::is_profile")]
    pub target: ProviderTarget,
    pub binding_id: BindingId,
    pub level: ConformanceLevel,
    /// Server-negotiated capabilities retained with the existing binding write.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
}

/// Generic provider target. Instance selection and authorization stay with the
/// provider; the gateway only retains and forwards this declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum ProviderTarget {
    Profile {},
    ExistingAgent { agent_id: String },
}

impl Default for ProviderTarget {
    fn default() -> Self {
        Self::Profile {}
    }
}

impl ProviderTarget {
    pub fn is_profile(&self) -> bool {
        matches!(self, Self::Profile {})
    }
    pub fn validate(&self) -> Result<(), ValidationError> {
        if let Self::ExistingAgent { agent_id } = self {
            require_string(agent_id, "/provider/target/agent_id", 1, 128)?;
            if !agent_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            {
                return Err(ValidationError::new(
                    "/provider/target/agent_id",
                    "must be a provider agent identifier",
                ));
            }
        }
        Ok(())
    }
}
