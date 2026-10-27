use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::profiles::store::{
    AgentProfileStore, InMemoryProfileStore, ProfilePage, ProfilePageRequest, ProfileStoreError,
    ProfileUseRecorder,
};
use crate::profiles::{
    AgentProfile, AgentProfileContent, AgentProfileRef, AgentProfileRevision, AgentProvider,
    ModelAccess, PermissionLevel, ProfileVisibility, SandboxProfileContent, SandboxProfileSnapshot,
    ThinkingEffort, WorkspaceLocation, WorkspaceProfileContent, WorkspaceProfileSnapshot,
};
use crate::sandbox::store::{InMemorySandboxStore, SandboxStore, WorkspaceStore};
use crate::{
    AgentId, AgentProfileId, Authorization, Caller, Capability, RuntimeCapabilities,
    RuntimeCapability, SandboxId, SandboxProfileId, SubjectId, WorkspaceId, WorkspaceProfileId,
};
use layer3_adapter::ConversationInputSender;

use super::super::*;

pub(super) const NOW: &str = "2026-09-05T00:00:00Z";

pub(super) fn agent_capabilities() -> RuntimeCapabilities {
    RuntimeCapabilities::new([
        RuntimeCapability::Conversation,
        RuntimeCapability::Steer,
        RuntimeCapability::Interrupt,
        RuntimeCapability::Usage,
        RuntimeCapability::Resources,
    ])
}

#[derive(Default)]
pub(super) struct TestAgentRuntimeRegistry {
    active: AtomicBool,
    fences: Mutex<Vec<(AgentId, u64)>>,
}

impl TestAgentRuntimeRegistry {
    pub(super) fn new(active: bool) -> Arc<Self> {
        Arc::new(Self {
            active: AtomicBool::new(active),
            fences: Mutex::new(Vec::new()),
        })
    }

    pub(super) fn fences(&self) -> Vec<(AgentId, u64)> {
        self.fences.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl AgentRuntimeRegistry for TestAgentRuntimeRegistry {
    async fn resolve(
        &self,
        _target: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        if !self.active.load(Ordering::SeqCst) {
            return Err(AgentRuntimeError::NotFound);
        }
        Ok(AgentRuntimeConnection {
            capabilities: RuntimeCapabilities::default(),
            input: ConversationInputSender::spawn(1, |_| async { Ok(()) }),
        })
    }

    async fn fence(
        &self,
        agent_id: &AgentId,
        authority_fence: u64,
    ) -> Result<(), AgentRuntimeError> {
        self.active.store(false, Ordering::SeqCst);
        self.fences
            .lock()
            .unwrap()
            .push((agent_id.clone(), authority_fence));
        Ok(())
    }
}

pub(super) fn caller(subject: &str, allowed: bool) -> Caller {
    Caller::new(
        SubjectId::new(subject),
        Authorization::new(allowed.then_some(Capability::UseOwnSandboxes)),
    )
}

pub(super) fn launch_caller(subject: &str, authority: Capability) -> Caller {
    Caller::new(SubjectId::new(subject), Authorization::new([authority]))
}

#[derive(Clone, Copy)]
pub(super) enum AgentProfileCorruption {
    RevisionProfileId,
    RevisionDigest,
}

struct CorruptingAgentProfiles {
    inner: Arc<InMemoryProfileStore>,
    corruption: AgentProfileCorruption,
}

#[async_trait::async_trait]
impl AgentProfileStore for CorruptingAgentProfiles {
    async fn create_agent_profile(&self, profile: AgentProfile) -> Result<(), ProfileStoreError> {
        self.inner.create_agent_profile(profile).await
    }

    async fn agent_profile(
        &self,
        id: &AgentProfileId,
    ) -> Result<Option<AgentProfile>, ProfileStoreError> {
        self.inner.agent_profile(id).await
    }

    async fn save_agent_draft(
        &self,
        expected_draft_version: u64,
        profile: AgentProfile,
    ) -> Result<(), ProfileStoreError> {
        self.inner
            .save_agent_draft(expected_draft_version, profile)
            .await
    }

    async fn publish_agent_profile(
        &self,
        id: &AgentProfileId,
        expected_draft_version: u64,
        published_at: String,
    ) -> Result<AgentProfileRevision, ProfileStoreError> {
        self.inner
            .publish_agent_profile(id, expected_draft_version, published_at)
            .await
    }

    async fn agent_profile_revision(
        &self,
        id: &AgentProfileId,
        revision: u32,
    ) -> Result<Option<AgentProfileRevision>, ProfileStoreError> {
        let mut stored = self.inner.agent_profile_revision(id, revision).await?;
        if let Some(stored) = &mut stored {
            match self.corruption {
                AgentProfileCorruption::RevisionProfileId => {
                    stored.profile_id = AgentProfileId("agp_wrong_revision".into());
                }
                AgentProfileCorruption::RevisionDigest => stored.digest = "wrong-digest".into(),
            }
        }
        Ok(stored)
    }

    async fn list_agent_profile_revisions(
        &self,
        id: &AgentProfileId,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<AgentProfileRevision>, ProfileStoreError> {
        self.inner.list_agent_profile_revisions(id, page).await
    }

    async fn list_agent_profiles(
        &self,
        owner: Option<&SubjectId>,
        public_only: bool,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<AgentProfile>, ProfileStoreError> {
        self.inner
            .list_agent_profiles(owner, public_only, page)
            .await
    }

    async fn retire_agent_profile(
        &self,
        id: &AgentProfileId,
        owner: &SubjectId,
        retired_at: String,
    ) -> Result<(), ProfileStoreError> {
        self.inner.retire_agent_profile(id, owner, retired_at).await
    }
}

pub(super) fn service_with_profile_corruption(
    store: Arc<InMemorySandboxStore>,
    profiles: Arc<InMemoryProfileStore>,
    corruption: AgentProfileCorruption,
) -> Arc<AgentService> {
    let profile_store: Arc<dyn AgentProfileStore> = Arc::new(CorruptingAgentProfiles {
        inner: profiles.clone(),
        corruption,
    });
    let profile_uses: Arc<dyn ProfileUseRecorder> = profiles;
    AgentService::with_dependencies(
        store,
        profile_store,
        profile_uses,
        TestAgentRuntimeRegistry::new(false),
        agent_capabilities(),
    )
}

pub(super) fn profile(id: &str, owner: &str, visibility: ProfileVisibility) -> AgentProfile {
    AgentProfile {
        internal: false,
        id: AgentProfileId(id.into()),
        owner: SubjectId::new(owner),
        name: "Research agent".into(),
        description: "Checks a repository".into(),
        visibility,
        forked_from: None,
        uses_count: 0,
        draft: AgentProfileContent {
            isolated_agent: false,
            model: Some("model-a".into()),
            thinking_effort: ThinkingEffort::High,
            permission_level: PermissionLevel::Guarded,
            skills: vec![],
            system_prompt: "Be exact.".into(),
        },
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    }
}

async fn publish(profiles: &InMemoryProfileStore, profile: AgentProfile) -> AgentProfileRef {
    let id = profile.id.clone();
    profiles.create_agent_profile(profile).await.unwrap();
    let revision = profiles
        .publish_agent_profile(&id, 1, NOW.into())
        .await
        .unwrap();
    AgentProfileRef::new(id, revision.revision)
}

fn sandbox(status: SandboxStatus, limit: u8) -> Sandbox {
    Sandbox {
        id: SandboxId("sbx_agent_service".into()),
        owner: SubjectId::new("owner"),
        name: "research".into(),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId("sbp_agent_service".into()),
            revision: 1,
            digest: "a".repeat(64),
            content: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1_000,
                memory_mib: 2_048,
                nyxid_services: vec![],
                agent_provider: AgentProvider::default(),
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 1,
                max_agents_per_workspace: limit,
                workspace_profiles: vec![],
                software_install_commands: vec![],
                environment_variables: vec![],
            },
        },
        status,
        failure: None,
        runtime: None,
        credential: SandboxCredentialState {
            source: Default::default(),
            service_grants: Vec::new(),
            service_scope: None,
            reaches: vec![],
            key_generation: 0,
            delivered: false,
            current: None,
            pending: None,
            revoke_debt: vec![],
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::None, NOW.into()),
        limits: SandboxLimits {
            max_workspaces: 1,
            max_agents_per_workspace: limit,
        },
        lifecycle: Default::default(),
        last_activity_at: NOW.into(),
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
        purge_after: None,
        revision: 1,
    }
}

pub(super) fn workspace(status: WorkspaceStatus) -> Workspace {
    Workspace {
        id: WorkspaceId("wks_agent_service".into()),
        sandbox_id: SandboxId("sbx_agent_service".into()),
        owner: SubjectId::new("owner"),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId("wkp_agent_service".into()),
            revision: 1,
            digest: "b".repeat(64),
            content: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory {
                    name: "project".into(),
                },
                name: "project".into(),
                setup_commands: vec![],
                agent_profiles: vec![],
            },
        },
        name: "project".into(),
        path: "/home/cma/project".into(),
        source: WorkspaceSource::Empty,
        resolved_commit: (status == WorkspaceStatus::Ready).then(|| "a".repeat(40)),
        status,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into()),
        next_worktree_number: 1,
        created_at: NOW.into(),
        ready_at: None,
        failed: None,
        removed_at: None,
        revision: 1,
    }
}

pub(super) async fn setup(
    status: SandboxStatus,
    workspace_status: WorkspaceStatus,
    limit: u8,
    profile_owner: &str,
    visibility: ProfileVisibility,
) -> (
    Arc<AgentService>,
    Arc<InMemorySandboxStore>,
    Arc<InMemoryProfileStore>,
    AgentProfileRef,
) {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let reference = publish(
        &profiles,
        profile("agp_agent_service", profile_owner, visibility),
    )
    .await;
    let store = Arc::new(InMemorySandboxStore::default());
    store.create_sandbox(sandbox(status, limit)).await.unwrap();
    store
        .create_workspace(workspace(workspace_status))
        .await
        .unwrap();
    (
        AgentService::new(
            store.clone(),
            profiles.clone(),
            TestAgentRuntimeRegistry::new(false),
            agent_capabilities(),
        ),
        store,
        profiles,
        reference,
    )
}

pub(super) fn request(profile: AgentProfileRef, origin: AgentOrigin) -> CreateAgent {
    CreateAgent {
        profile,
        title: Some("Proof search".into()),
        first_message: Some("Inspect the proof.".into()),
        origin,
    }
}

pub(super) async fn setup_profile_children(
    status: SandboxStatus,
    workspace_status: WorkspaceStatus,
    count: u16,
) -> (
    Arc<AgentService>,
    Arc<InMemorySandboxStore>,
    Arc<InMemoryProfileStore>,
    Workspace,
) {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let mut workspace = workspace(workspace_status);
    for index in 0..count {
        workspace.profile.content.agent_profiles.push(
            publish(
                &profiles,
                profile(
                    &format!("agp_profile_{index}"),
                    "owner",
                    ProfileVisibility::Private,
                ),
            )
            .await,
        );
    }
    workspace.profile.digest = crate::profiles::content_digest(&workspace.profile.content).unwrap();
    let store = Arc::new(InMemorySandboxStore::default());
    store.create_sandbox(sandbox(status, 4)).await.unwrap();
    store.create_workspace(workspace.clone()).await.unwrap();
    let service = AgentService::new(
        store.clone(),
        profiles.clone(),
        TestAgentRuntimeRegistry::new(false),
        agent_capabilities(),
    );
    (service, store, profiles, workspace)
}

pub(super) async fn uses(profiles: &InMemoryProfileStore, id: &AgentProfileId) -> u64 {
    profiles
        .agent_profile(id)
        .await
        .unwrap()
        .unwrap()
        .uses_count
}
