//! Shared hierarchy metadata without owner configuration or repository paths.
use super::*;
use serde::{Deserialize, Deserializer};

string_enum! { /// Shared hierarchy projection marker.
    pub enum SharedHierarchyAccess { Shared => "shared" }
}

/// Shared Sandbox summary and authorized child counts.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SharedSandboxSummary {
    /// Explicit shared projection.
    pub access: SharedHierarchyAccess,
    /// CMA Sandbox identity.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Durable lifecycle status.
    pub status: SandboxStatus,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
    /// Last activity timestamp.
    pub last_activity_at: String,
    /// Retirement timestamp, if retired.
    pub retired_at: Option<String>,
    /// Authorized Workspace count.
    pub workspace_count: u64,
    /// Authorized Worktree count.
    pub worktree_count: u64,
    /// Authorized Agent count.
    pub agent_count: u64,
}
/// Shared Sandbox detail and authorized child counts.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SharedSandboxDetail {
    /// Explicit shared projection.
    pub access: SharedHierarchyAccess,
    /// CMA Sandbox identity.
    pub id: String,
    /// Current Sandbox revision. Older servers may omit this field.
    #[serde(default, deserialize_with = "optional_revision")]
    pub revision: Option<u64>,
    /// Display name.
    pub name: String,
    /// Durable lifecycle status.
    pub status: SandboxStatus,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
    /// Last activity timestamp.
    pub last_activity_at: String,
    /// Retirement timestamp, if retired.
    pub retired_at: Option<String>,
    /// Authorized Workspace count.
    pub workspace_count: u64,
    /// Authorized Worktree count.
    pub worktree_count: u64,
    /// Authorized Agent count.
    pub agent_count: u64,
    /// Safe child Workspace summaries.
    pub workspaces: Vec<SharedWorkspaceSummary>,
}
/// Shared Workspace summary without its profile identity.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SharedWorkspaceSummary {
    /// Explicit shared projection.
    pub access: SharedHierarchyAccess,
    /// CMA Workspace identity.
    pub id: String,
    /// Independently authorized parent Sandbox.
    pub sandbox_id: String,
    /// Display name.
    pub name: String,
    /// Durable lifecycle status.
    pub status: WorkspaceStatus,
    /// Authorized Worktree count.
    pub worktree_count: u64,
    /// Authorized Agent count.
    pub agent_count: u64,
}
/// Shared Workspace detail without repository location or profile configuration.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SharedWorkspaceDetail {
    /// Shared identity, status and counts.
    #[serde(flatten)]
    pub summary: SharedWorkspaceSummary,
    /// Creation timestamp.
    pub created_at: String,
    /// Readiness timestamp, if ready.
    pub ready_at: Option<String>,
    /// Removal timestamp, if removed.
    pub removed_at: Option<String>,
    /// Safe Worktree summaries.
    pub worktrees: Vec<SharedWorktree>,
    /// Safe Agent summaries in this authorized Workspace.
    pub agents: Vec<SharedHierarchyAgent>,
}
/// Shared Worktree metadata without its path, branch or base commit.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SharedWorktree {
    /// Explicit shared projection.
    pub access: SharedHierarchyAccess,
    /// CMA Worktree identity.
    pub id: String,
    /// Authorized parent Workspace.
    pub workspace_id: String,
    /// Authorized parent Sandbox.
    pub sandbox_id: String,
    /// Display name.
    pub name: String,
    /// Durable lifecycle status.
    pub status: WorktreeStatus,
    /// Assigned Agent, when present.
    pub agent_id: Option<String>,
}
/// Agent metadata shown inside an independently authorized Workspace.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SharedHierarchyAgent {
    /// CMA Agent identity.
    pub id: String,
    /// Authorized Worktree identity.
    pub worktree_id: String,
    /// Authorized Workspace identity.
    pub workspace_id: String,
    /// Authorized Sandbox identity.
    pub sandbox_id: String,
    /// Durable Agent status.
    pub status: AgentStatus,
    /// Agent title, when present.
    pub title: Option<String>,
    /// Last update timestamp.
    pub updated_at: String,
}

macro_rules! accessible {
    ($name:ident, $personal:ty, $shared:ty, $($personal_id:ident).+, $($shared_id:ident).+) => {
        /// Personal hierarchy detail or an explicit safe shared projection.
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum $name {
            /// Existing full personal projection.
            Personal(Box<$personal>),
            /// Shared metadata without private configuration.
            Shared($shared),
        }
        impl $name {
            /// CMA resource identity in either projection.
            pub fn id(&self) -> &str {
                match self { Self::Personal(value) => value.$($personal_id).+.as_str(), Self::Shared(value) => value.$($shared_id).+.as_str() }
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
                let value = serde_json::Value::deserialize(deserializer)?;
                let result = match value.get("access") {
                    None => serde_json::from_value(value).map(Self::Personal),
                    Some(marker) if marker == "shared" => serde_json::from_value(value).map(Self::Shared),
                    Some(_) => return Err(serde::de::Error::custom("unknown hierarchy access discriminator")),
                };
                result.map_err(serde::de::Error::custom)
            }
        }
    }
}
accessible!(
    AccessibleSandboxDetail,
    SandboxDetail,
    Box<SharedSandboxDetail>,
    summary.sandbox.id,
    id
);
accessible!(
    AccessibleWorkspaceDetail,
    WorkspaceDetail,
    Box<SharedWorkspaceDetail>,
    workspace.id,
    summary.id
);
accessible!(
    AccessibleWorkspaceSummary,
    WorkspaceSummary,
    SharedWorkspaceSummary,
    id,
    id
);
accessible!(AccessibleWorktree, Worktree, SharedWorktree, id, id);

accessible!(
    AccessibleSandboxSummary,
    SandboxSummary,
    SharedSandboxSummary,
    sandbox.id,
    id
);

fn optional_revision<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<u64>, D::Error> {
    let revision = u64::deserialize(deserializer)?;
    if revision == 0 || revision > i64::MAX as u64 {
        return Err(serde::de::Error::custom("invalid shared Sandbox revision"));
    }
    Ok(Some(revision))
}
