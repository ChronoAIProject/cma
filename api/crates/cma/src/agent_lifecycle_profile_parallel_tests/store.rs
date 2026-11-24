use super::*;
use std::sync::Mutex;
use tokio::sync::Notify;

#[derive(Default)]
pub(super) struct Store {
    pub inner: InMemoryProfileStore,
    pub events: Mutex<Vec<(&'static str, bool)>>,
    failures: Mutex<Vec<(&'static str, bool, ProfileStoreError)>>,
    // Agent insertion cannot finish before the whole other branch publishes.
    pub hold_agent_until_sandbox: bool,
    sandbox_published: Notify,
}

impl Store {
    pub fn blocking_agent() -> Self {
        Self {
            hold_agent_until_sandbox: true,
            ..Self::default()
        }
    }

    pub fn fail(&self, method: &'static str, after: bool, error: ProfileStoreError) {
        self.failures.lock().unwrap().push((method, after, error));
    }

    async fn observe(&self, method: &'static str, after: bool) -> Result<(), ProfileStoreError> {
        self.events.lock().unwrap().push((method, after));
        let failure = {
            let mut failures = self.failures.lock().unwrap();
            failures
                .iter()
                .position(|(name, phase, _)| *name == method && *phase == after)
                .map(|index| failures.remove(index).2)
        };
        if let Some(error) = failure {
            return Err(error);
        }
        if self.hold_agent_until_sandbox {
            if method == "create_agent_profile" && !after {
                self.sandbox_published.notified().await;
            } else if method == "ensure_initial_sandbox_profile" && after {
                self.sandbox_published.notify_one();
            }
        }
        Ok(())
    }

    pub fn completed(&self, method: &str) -> usize {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|(name, after)| *name == method && *after)
            .count()
    }
}

// Instrument the real private-profile service at its storage boundary. All
// validation, reference checks, identity derivation and replay stay in Core.
macro_rules! delegate {
    ($trait:ident { $(fn $method:ident($($arg:ident: $ty:ty),*) -> $result:ty;)* }) => {
        #[async_trait::async_trait]
        impl $trait for Store {
            $(async fn $method(&self, $($arg: $ty),*) -> Result<$result, ProfileStoreError> {
                self.observe(stringify!($method), false).await?;
                let value = self.inner.$method($($arg),*).await?;
                self.observe(stringify!($method), true).await?;
                Ok(value)
            })*
        }
    };
}

delegate!(AgentProfileStore {
    fn create_agent_profile(profile: AgentProfile) -> ();
    fn agent_profile(id: &AgentProfileId) -> Option<AgentProfile>;
    fn save_agent_draft(expected: u64, profile: AgentProfile) -> ();
    fn ensure_initial_agent_profile(profile: &AgentProfile, at: String) -> AgentProfileRevision;
    fn publish_agent_profile(id: &AgentProfileId, expected: u64, at: String) -> AgentProfileRevision;
    fn agent_profile_revision(id: &AgentProfileId, revision: u32) -> Option<AgentProfileRevision>;
    fn list_agent_profile_revisions(id: &AgentProfileId, page: &ProfilePageRequest) -> ProfilePage<AgentProfileRevision>;
    fn list_agent_profiles(owner: Option<&SubjectId>, public_only: bool, page: &ProfilePageRequest) -> ProfilePage<AgentProfile>;
    fn retire_agent_profile(id: &AgentProfileId, owner: &SubjectId, at: String) -> ();
});

delegate!(WorkspaceProfileStore {
    fn create_workspace_profile(profile: WorkspaceProfile) -> ();
    fn workspace_profile(id: &WorkspaceProfileId) -> Option<WorkspaceProfile>;
    fn save_workspace_draft(expected: u64, profile: WorkspaceProfile) -> ();
    fn ensure_initial_workspace_profile(profile: &WorkspaceProfile, at: String) -> WorkspaceProfileRevision;
    fn publish_workspace_profile(id: &WorkspaceProfileId, expected: u64, at: String) -> WorkspaceProfileRevision;
    fn workspace_profile_revision(id: &WorkspaceProfileId, revision: u32) -> Option<WorkspaceProfileRevision>;
    fn list_workspace_profile_revisions(id: &WorkspaceProfileId, page: &ProfilePageRequest) -> ProfilePage<WorkspaceProfileRevision>;
    fn list_workspace_profiles(owner: Option<&SubjectId>, public_only: bool, page: &ProfilePageRequest) -> ProfilePage<WorkspaceProfile>;
    fn retire_workspace_profile(id: &WorkspaceProfileId, owner: &SubjectId, at: String) -> ();
});

delegate!(SandboxProfileStore {
    fn create_sandbox_profile(profile: SandboxProfile) -> ();
    fn sandbox_profile(id: &SandboxProfileId) -> Option<SandboxProfile>;
    fn save_sandbox_draft(expected: u64, profile: SandboxProfile) -> ();
    fn ensure_initial_sandbox_profile(profile: &SandboxProfile, at: String, bounds: ProfileCapacityBounds, scanner: &dyn PublicValueScanner) -> SandboxProfileRevision;
    fn publish_sandbox_profile(id: &SandboxProfileId, expected: u64, at: String, bounds: ProfileCapacityBounds, scanner: &dyn PublicValueScanner) -> SandboxProfileRevision;
    fn sandbox_profile_revision(id: &SandboxProfileId, revision: u32) -> Option<SandboxProfileRevision>;
    fn list_sandbox_profile_revisions(id: &SandboxProfileId, page: &ProfilePageRequest) -> ProfilePage<SandboxProfileRevision>;
    fn list_sandbox_profiles(owner: Option<&SubjectId>, public_only: bool, page: &ProfilePageRequest) -> ProfilePage<SandboxProfile>;
    fn retire_sandbox_profile(id: &SandboxProfileId, owner: &SubjectId, at: String) -> ();
});
