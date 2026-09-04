//! Versioned configuration for sandboxes, workspaces, and agents.

mod admin;
mod agent;
mod common;
mod digest;
mod marketplace;
mod sandbox;
mod validation;
mod workspace;

pub mod store;

pub use admin::{
    ProfileAdminQuery, ProfileAdminStore, ProfileAdminSummary, ProfileKind, ProfileLocator,
};
pub use agent::{
    AgentProfile, AgentProfileContent, AgentProfileRevision, AgentProfileSnapshot, PermissionLevel,
    SkillRef, SkillSource, ThinkingEffort,
};
pub use common::{
    AgentProfileRef, ProfileError, ProfileVisibility, SandboxProfileRef, WorkspaceProfileRef,
};
pub use digest::content_digest;
pub use marketplace::{
    require_trust_confirmation, ForkedProfile, MarketplaceEntry, MarketplaceError, MarketplaceKind,
    MarketplaceLocator, MarketplacePage, MarketplaceQuery, MarketplaceRevision, MarketplaceService,
    MarketplaceStore, MarketplaceSummary, ResolvedProfileName, TrustConfirmation,
};
pub use sandbox::{
    AgentProvider, EnvironmentVariable, ModelAccess, ProfileCapacityBounds, SandboxProfile,
    SandboxProfileContent, SandboxProfileRevision, SandboxProfileSnapshot,
};
pub use store::{
    AgentProfileStore, InMemoryProfileStore, ProfilePage, ProfilePageRequest, ProfileStoreError,
    SandboxProfileStore, WorkspaceProfileStore,
};
pub use validation::{validate_command, validate_workspace_name};
pub use workspace::{
    WorkspaceLocation, WorkspaceProfile, WorkspaceProfileContent, WorkspaceProfileRevision,
    WorkspaceProfileSnapshot,
};

#[cfg(test)]
mod tests;
