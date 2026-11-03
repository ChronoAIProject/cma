//! Immutable creation facts, separately authorized from current source shortcuts.
use crate::resource_access::ResourceActor;
use serde::Deserialize;

/// Personal, organization or shared creation inventory, with optional category filtering.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentProvenanceInventoryOptions {
    #[doc = "Filter the authorized task outcome independently of lifecycle status."]
    pub task_status: Option<crate::instances::AgentTaskStatus>,
    /// Explicit incoming shares across current organizations; excludes personal
    /// ownership. Mutually exclusive with `scope.organization`.
    pub shared: bool,
    /// Organization, status, parent and page filters.
    pub scope: super::AccessibleAgentListOptions,
    /// Original category. Follow `next` even when a filtered page is empty.
    pub creation_origin: Option<AgentCreationCategory>,
    /// Filter explicitly active channel bindings; independent of creation origin.
    pub channel_active: Option<bool>,
    /// Filter retained channel attachment/use history, including detached bindings.
    pub channel_history: Option<bool>,
}

/// Safe creation summary without private source or creator references.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentProvenanceInventoryItem {
    #[doc = "Authorized task outcome; absent on older servers."]
    pub task_status: Option<crate::instances::AgentTaskStatus>,
    /// CMA Agent identity.
    pub agent_id: String,
    /// Authorized Agent title when present.
    pub title: Option<String>,
    /// Current recorded lifecycle status, including retained deleted Agents.
    pub status: crate::instances::AgentStatus,
    /// Original creation time.
    pub created_at: String,
    /// Latest recorded update time.
    pub updated_at: String,
    /// Immutable creation category.
    pub creation_origin: AgentCreationCategory,
    /// Safe explicit channel facts; absent when reading an older server.
    pub channel_usage: Option<super::ChannelUsageSummary>,
}

string_enum! { /// Original creation category; later use never relabels it.
    pub enum AgentCreationCategory {
        Manual => "manual", Trigger => "trigger", ChannelBridge => "channel_bridge", LegacyUnknown => "legacy_unknown"
    }
}
/// Immutable creation information under current resource access.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentProvenance {
    /// CMA Agent identity.
    pub agent_id: String,
    /// Original creation timestamp.
    pub created_at: String,
    /// Category supported by creation evidence.
    pub creation_origin: AgentCreationCategory,
    /// Only resource managers receive attribution and private source references.
    pub details: Option<AgentProvenanceDetails>,
}
/// Source attribution for an authorized resource manager.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentProvenanceDetails {
    /// Creating actor when captured; never inferred from current ownership.
    pub created_by: Option<ResourceActor>,
    /// Original Trigger launch, when applicable.
    pub trigger: Option<AgentCreationTrigger>,
    /// Original channel allocation, when applicable.
    pub channel_bridge: Option<AgentCreationBridge>,
    /// Original automatic Workspace profile slot, when applicable.
    pub workspace_profile_index: Option<u16>,
}
/// Original Trigger creation evidence; unrelated to later reuse.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentCreationTrigger {
    /// Original Trigger identity.
    pub trigger_id: String,
    /// Original launch identity.
    pub launch_id: String,
    /// Captured launching principal, when known.
    pub launching_actor: Option<ResourceActor>,
}
/// Original channel creation references.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentCreationBridge {
    /// Creating bridge identity.
    pub bridge_id: String,
    /// Creating partition identity.
    pub partition_id: String,
    /// Original binding generation.
    pub generation: u64,
    /// External source reference available only to resource managers.
    pub source_reference: String,
}
