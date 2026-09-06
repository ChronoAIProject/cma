use async_trait::async_trait;
use layer2_core::profiles::*;
use layer2_core::{AgentProfileId, SandboxProfileId, SubjectId, WorkspaceProfileId};

pub(super) struct UnavailableProfiles;

macro_rules! unavailable_port {
    ($port:ident { $(fn $method:ident($($arg:ident: $kind:ty),*) -> $result:ty, $error:ty;)* }) => {
        #[async_trait]
        impl $port for UnavailableProfiles {
            $(async fn $method(&self, $($arg: $kind),*) -> Result<$result, $error> {
                let _ = ($($arg,)*);
                Err(<$error>::Unavailable("contract profile storage unavailable".into()))
            })*
        }
    };
}

unavailable_port!(AgentProfileStore {
    fn create_agent_profile(profile: AgentProfile) -> (), ProfileStoreError;
    fn agent_profile(id: &AgentProfileId) -> Option<AgentProfile>, ProfileStoreError;
    fn save_agent_draft(version: u64, profile: AgentProfile) -> (), ProfileStoreError;
    fn publish_agent_profile(id: &AgentProfileId, version: u64, at: String) -> AgentProfileRevision, ProfileStoreError;
    fn agent_profile_revision(id: &AgentProfileId, revision: u32) -> Option<AgentProfileRevision>, ProfileStoreError;
    fn list_agent_profile_revisions(id: &AgentProfileId, page: &ProfilePageRequest) -> ProfilePage<AgentProfileRevision>, ProfileStoreError;
    fn list_agent_profiles(owner: Option<&SubjectId>, public_only: bool, page: &ProfilePageRequest) -> ProfilePage<AgentProfile>, ProfileStoreError;
    fn retire_agent_profile(id: &AgentProfileId, owner: &SubjectId, at: String) -> (), ProfileStoreError;
});

unavailable_port!(WorkspaceProfileStore {
    fn create_workspace_profile(profile: WorkspaceProfile) -> (), ProfileStoreError;
    fn workspace_profile(id: &WorkspaceProfileId) -> Option<WorkspaceProfile>, ProfileStoreError;
    fn save_workspace_draft(version: u64, profile: WorkspaceProfile) -> (), ProfileStoreError;
    fn publish_workspace_profile(id: &WorkspaceProfileId, version: u64, at: String) -> WorkspaceProfileRevision, ProfileStoreError;
    fn workspace_profile_revision(id: &WorkspaceProfileId, revision: u32) -> Option<WorkspaceProfileRevision>, ProfileStoreError;
    fn list_workspace_profile_revisions(id: &WorkspaceProfileId, page: &ProfilePageRequest) -> ProfilePage<WorkspaceProfileRevision>, ProfileStoreError;
    fn list_workspace_profiles(owner: Option<&SubjectId>, public_only: bool, page: &ProfilePageRequest) -> ProfilePage<WorkspaceProfile>, ProfileStoreError;
    fn retire_workspace_profile(id: &WorkspaceProfileId, owner: &SubjectId, at: String) -> (), ProfileStoreError;
});

unavailable_port!(SandboxProfileStore {
    fn create_sandbox_profile(profile: SandboxProfile) -> (), ProfileStoreError;
    fn sandbox_profile(id: &SandboxProfileId) -> Option<SandboxProfile>, ProfileStoreError;
    fn save_sandbox_draft(version: u64, profile: SandboxProfile) -> (), ProfileStoreError;
    fn publish_sandbox_profile(id: &SandboxProfileId, version: u64, at: String, bounds: ProfileCapacityBounds, scanner: &dyn PublicValueScanner) -> SandboxProfileRevision, ProfileStoreError;
    fn sandbox_profile_revision(id: &SandboxProfileId, revision: u32) -> Option<SandboxProfileRevision>, ProfileStoreError;
    fn list_sandbox_profile_revisions(id: &SandboxProfileId, page: &ProfilePageRequest) -> ProfilePage<SandboxProfileRevision>, ProfileStoreError;
    fn list_sandbox_profiles(owner: Option<&SubjectId>, public_only: bool, page: &ProfilePageRequest) -> ProfilePage<SandboxProfile>, ProfileStoreError;
    fn retire_sandbox_profile(id: &SandboxProfileId, owner: &SubjectId, at: String) -> (), ProfileStoreError;
});

unavailable_port!(MarketplaceStore {
    fn list_marketplace(query: &MarketplaceQuery) -> MarketplacePage, MarketplaceError;
    fn read_marketplace(locator: &MarketplaceLocator) -> MarketplaceEntry, MarketplaceError;
    fn set_profile_visibility(owner: &SubjectId, locator: &MarketplaceLocator, visibility: ProfileVisibility, at: String, bounds: ProfileCapacityBounds, scanner: &dyn PublicValueScanner) -> (), MarketplaceError;
    fn fork_profile(owner: &SubjectId, locator: &MarketplaceLocator, revision: u32, at: String) -> ForkedProfile, MarketplaceError;
});

unavailable_port!(ProfileAdminStore {
    fn list_profiles(query: &ProfileAdminQuery) -> ProfilePage<ProfileAdminSummary>, ProfileStoreError;
    fn retire_profile(locator: &ProfileLocator, at: String) -> ProfileAdminSummary, ProfileStoreError;
});
