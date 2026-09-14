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
    Shared(SharedAgent),
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
