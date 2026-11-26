//! Versioned configuration for sandboxes, workspaces, and agents.

mod admin;
mod agent;
pub mod authoring;
mod availability;
mod common;
mod connections;
mod digest;
mod marketplace;
mod owned_creation;
pub use owned_creation::{
    private_agent_profile_id, private_sandbox_profile_id, private_workspace_profile_id,
    PrivateProfileCreationService, PrivateSandboxProfileInput, ProfileCreationError,
};
mod public_values;
pub mod references;
mod sandbox;
pub use availability::SandboxProfileAvailability;
pub(crate) use sandbox::valid_runtime_image;
mod validation;
mod workspace;

pub mod store;

#[cfg(any(test, feature = "test-support"))]
pub mod initial_publication_contract;

pub use admin::{
    ProfileAdminQuery, ProfileAdminStore, ProfileAdminSummary, ProfileKind, ProfileLocator,
};
pub use agent::{
    AgentProfile, AgentProfileContent, AgentProfileRevision, AgentProfileSnapshot, PermissionLevel,
    SkillRef, SkillSource, ThinkingEffort,
};
pub use common::{
    AgentProfileRef, ProfileError, ProfileVisibility, SandboxProfileRef, WorkspaceProfileRef,
    MAX_PROFILE_NAME_BYTES,
};
pub use digest::content_digest;
pub use marketplace::{
    require_trust_confirmation, ForkedProfile, MarketplaceEntry, MarketplaceError, MarketplaceKind,
    MarketplaceLocator, MarketplacePage, MarketplaceQuery, MarketplaceRevision, MarketplaceService,
    MarketplaceStore, MarketplaceSummary, ResolvedProfileName, ResolvedProfileRevision,
    TrustConfirmation,
};
pub use public_values::{PublicValueScanError, PublicValueScanner};
pub use sandbox::{
    AgentProvider, EnvironmentVariable, ModelAccess, ProfileCapacityBounds, SandboxProfile,
    SandboxProfileContent, SandboxProfileRevision, SandboxProfileSnapshot,
};
pub use store::{
    AgentProfileStore, InMemoryProfileStore, ProfilePage, ProfilePageRequest, ProfileStoreError,
    ProfileUseRecorder, SandboxProfileStore, WorkspaceProfileStore,
};
pub use validation::{validate_command, validate_workspace_name};
pub use workspace::{
    WorkspaceLocation, WorkspaceProfile, WorkspaceProfileContent, WorkspaceProfileRevision,
    WorkspaceProfileSnapshot,
};

#[cfg(test)]
mod public_value_policy_tests;
#[cfg(test)]
mod tests;
