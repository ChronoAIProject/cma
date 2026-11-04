//! Explicit Agent-only detail for shared and organization access.
use super::Agent;
use crate::instances::AgentStatus;
use serde::{Deserialize, Deserializer};

string_enum! { /// Access projection marker for nonpersonal readers.
    pub enum SharedAgentAccess { Shared => "shared" }
}

/// Safe shared Agent detail without parent configuration or runtime identity.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SharedAgent {
    /// Browser page for the actual CMA Agent, when available.
    #[serde(default)]
    pub callback_url: Option<String>,
    /// Explicit shared projection marker.
    pub access: SharedAgentAccess,
    /// CMA Agent identifier.
    pub id: String,
    /// Current durable status.
    pub status: AgentStatus,
    /// Current Agent title.
    pub title: Option<String>,
    /// Configured model name, if any.
    pub model: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
    /// Last durable update timestamp.
    pub updated_at: String,
    /// Readiness timestamp, if available.
    pub ready_at: Option<String>,
    /// Stop timestamp, if available.
    pub stopped_at: Option<String>,
    /// Deletion timestamp, if available.
    pub deleted_at: Option<String>,
}

/// The existing full personal response or an explicitly shared safe response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessibleAgent {
    /// Complete owner detail including parent context.
    Personal(Box<Agent>),
    /// Agent-only detail for other authorized readers.
    Shared(Box<SharedAgent>),
}
impl AccessibleAgent {
    /// The CMA Agent identity in either projection.
    pub fn id(&self) -> &str {
        match self {
            Self::Personal(value) => &value.id,
            Self::Shared(value) => &value.id,
        }
    }
}
impl<'de> Deserialize<'de> for AccessibleAgent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let result = match value.get("access") {
            None => serde_json::from_value(value).map(Self::Personal),
            Some(marker) if marker == "shared" => serde_json::from_value(value).map(Self::Shared),
            Some(_) => {
                return Err(serde::de::Error::custom(
                    "unknown Agent access discriminator",
                ));
            }
        };
        result.map_err(serde::de::Error::custom)
    }
}

/// A personal summary or the safe shared Agent-only projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessibleAgentSummary {
    /// Existing personal list summary including parent context.
    Personal(Box<super::AgentSummary>),
    /// Shared summary without parent or profile context.
    Shared(Box<SharedAgent>),
}
impl<'de> Deserialize<'de> for AccessibleAgentSummary {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let result = match value.get("access") {
            None => serde_json::from_value(value).map(Self::Personal),
            Some(marker) if marker == "shared" => serde_json::from_value(value).map(Self::Shared),
            Some(_) => {
                return Err(serde::de::Error::custom(
                    "unknown Agent access discriminator",
                ));
            }
        };
        result.map_err(serde::de::Error::custom)
    }
}

/// Creation category without private Launch/Trigger IDs or profile indexes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SharedAgentOrigin {
    /// Direct creation.
    Manual,
    /// Created as a configured Workspace child.
    WorkspaceProfile,
    /// Created through a Trigger Launch.
    Trigger,
}

/// Safe shared source; publication and private launch data are withheld.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SharedAgentSource {
    /// Explicit shared projection marker.
    pub access: SharedAgentAccess,
    /// CMA Agent identifier.
    pub agent_id: String,
    /// Original creation category.
    pub origin: SharedAgentOrigin,
}

/// Personal source details or the safe shared creation category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessibleAgentSource {
    /// Personal provenance and current published Trigger shortcuts.
    Personal(super::AgentSource),
    /// Agent-only category for other readers.
    Shared(SharedAgentSource),
}
impl AccessibleAgentSource {
    /// CMA Agent identifier in either projection.
    pub fn agent_id(&self) -> &str {
        match self {
            Self::Personal(value) => &value.agent_id,
            Self::Shared(value) => &value.agent_id,
        }
    }
}
impl<'de> Deserialize<'de> for AccessibleAgentSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let result = match value.get("access") {
            None => serde_json::from_value(value).map(Self::Personal),
            Some(marker) if marker == "shared" => serde_json::from_value(value).map(Self::Shared),
            Some(_) => {
                return Err(serde::de::Error::custom(
                    "unknown Agent source access discriminator",
                ));
            }
        };
        result.map_err(serde::de::Error::custom)
    }
}
