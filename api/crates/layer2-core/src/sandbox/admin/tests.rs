use std::sync::Mutex;

use crate::profiles::store::{AgentProfileStore, InMemoryProfileStore};
use crate::profiles::{
    AgentProfile, AgentProfileContent, AgentProfileSnapshot, AgentProvider, ModelAccess,
    PermissionLevel, ProfileKind, ProfileVisibility, SandboxProfileContent, SandboxProfileSnapshot,
    ThinkingEffort, WorkspaceLocation, WorkspaceProfileContent, WorkspaceProfileSnapshot,
};
use crate::sandbox::store::{
    AgentStore, InMemorySandboxStore, SandboxEventStore, SandboxStore, WorkspaceStore,
};
use crate::sandbox::{RuntimeBinding, SandboxOrigin};
use crate::{ActivityQuery, ActivityStore, Authorization, Page as ActivityPage, StoreError};

use super::*;

const NOW: &str = "2026-09-05T00:00:00Z";

#[derive(Default)]
struct MemoryActivity {
    records: Mutex<Vec<ActivityRecord>>,
}

#[async_trait]
impl ActivityStore for MemoryActivity {
    async fn append(&self, record: &ActivityRecord) -> Result<(), StoreError> {
        self.records.lock().unwrap().push(record.clone());
        Ok(())
    }

    async fn list(
        &self,
        _query: &ActivityQuery,
    ) -> Result<ActivityPage<ActivityRecord>, StoreError> {
        Ok(ActivityPage::new(
            self.records.lock().unwrap().clone(),
            None,
        ))
    }
}

struct Fixture {
    service: SandboxOperatorService,
    sandboxes: Arc<InMemorySandboxStore>,
    profiles: Arc<InMemoryProfileStore>,
    activity: Arc<MemoryActivity>,
}

fn fixture() -> Fixture {
    let sandboxes = Arc::new(InMemorySandboxStore::default());
    let profiles = Arc::new(InMemoryProfileStore::default());
    let activity = Arc::new(MemoryActivity::default());
    let retirement = Arc::new(StoredSandboxRetirement::new(sandboxes.clone()));
    let service = SandboxOperatorService::new(
        sandboxes.clone(),
        profiles.clone(),
        retirement,
        ActivityService::new(activity.clone()),
    )
    .with_maintenance(sandboxes.clone());
    Fixture {
        service,
        sandboxes,
        profiles,
        activity,
    }
}

fn caller(capabilities: impl IntoIterator<Item = Capability>) -> Caller {
    Caller::new(
        SubjectId::new("operator-a"),
        Authorization::new(capabilities),
    )
}

fn sandbox(id: &str, owner: &str, status: SandboxStatus, runtime_version: &str) -> Sandbox {
    Sandbox {
        id: SandboxId(id.into()),
        owner: SubjectId::new(owner),
        name: format!("sandbox-{id}"),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: crate::SandboxProfileId("sbp_test".into()),
            revision: 1,
            digest: "a".repeat(64),
            always_on: None,
            content: sandbox_profile_content(),
        },
        status,
        failure: None,
        runtime: Some(RuntimeBinding {
            boot_id: None,
            runtime_id: format!("runtime-{id}"),
            runtime_version: runtime_version.into(),
            provision_generation: None,
            bound_at: NOW.into(),
        }),
        credential: super::super::SandboxCredentialState {
            source: Default::default(),
            service_grants: Vec::new(),
            service_scope: None,
            reaches: vec![],
            key_generation: 0,
            delivered: false,
            current: None,
            pending: None,
            revoke_debt: Vec::new(),
            sign_in: super::super::SandboxSignInState::NotRequired,
        },
        work: super::super::SandboxWorkState::new(
            super::super::SandboxWorkStep::WaitForCredentialRotation,
            NOW.into(),
        ),
        limits: super::super::SandboxLimits {
            max_workspaces: 4,
            max_agents_per_workspace: 4,
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

fn sandbox_profile_content() -> SandboxProfileContent {
    SandboxProfileContent {
        runtime_image: None,
        cpu_millis: 1_000,
        memory_mib: 2_048,
        nyxid_services: vec![],
        agent_provider: AgentProvider::default(),
        model_access: ModelAccess::ChronoaiManaged,
        max_workspaces: 4,
        max_agents_per_workspace: 4,
        workspace_profiles: vec![],
        software_install_commands: vec![],
        environment_variables: vec![],
    }
}

fn workspace(sandbox: &Sandbox, id: &str) -> Workspace {
    Workspace {
        id: WorkspaceId(id.into()),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        origin: super::super::WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: crate::WorkspaceProfileId("wkp_test".into()),
            revision: 2,
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
        source: super::super::WorkspaceSource::Empty,
        resolved_commit: Some("a".repeat(40)),
        status: WorkspaceStatus::Ready,
        work: super::super::WorkspaceWorkState::new(
            super::super::WorkspaceWorkStep::None,
            NOW.into(),
        ),
        next_worktree_number: 1,
        created_at: NOW.into(),
        ready_at: Some(NOW.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    }
}

fn agent(sandbox: &Sandbox, workspace: &Workspace, id: &str, status: AgentStatus) -> Agent {
    Agent {
        id: crate::AgentId(id.into()),
        worktree_id: WorktreeId(format!("wkt_{id}")),
        workspace_id: workspace.id.clone(),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        creation_key: format!("create-{id}"),
        creation_fingerprint: "d".repeat(64),
        profile: AgentProfileSnapshot {
            id: crate::AgentProfileId("agp_test".into()),
            revision: 3,
            digest: "c".repeat(64),
            content: agent_profile_content(),
        },
        capabilities: crate::RuntimeCapabilities::new([crate::RuntimeCapability::Conversation]),
        status,
        conversation: super::super::ConversationState {
            turn_timeout: None,
            preferences: Default::default(),
            managed_guidance_revision: 0,
            observations: Default::default(),
            settings: None,
            conversation_id: Some("conversation-a".into()),
            runtime_generation: Some(1),
            last_seq: 0,
            first_message: None,
            active_turn: None,
            operation: None,
            queued_messages: vec![],
        },
        work: super::super::AgentWorkState::new(super::super::AgentWorkStep::None, NOW.into()),
        failure: None,
        title: Some("Proof search".into()),
        created_at: NOW.into(),
        updated_at: NOW.into(),
        ready_at: Some(NOW.into()),
        stopped_at: None,
        requested_stop_reason: None,
        stop_reason: None,
        deleted_at: None,
        origin: super::super::AgentOrigin::Manual,
        revision: 1,
    }
}

fn agent_profile_content() -> AgentProfileContent {
    AgentProfileContent {
        isolated_agent: false,
        model: None,
        thinking_effort: ThinkingEffort::Medium,
        permission_level: PermissionLevel::Guarded,
        skills: vec![],
        system_prompt: String::new(),
    }
}

fn agent_profile(id: &str, owner: &str, visibility: ProfileVisibility) -> AgentProfile {
    AgentProfile {
        internal: false,
        id: crate::AgentProfileId(id.into()),
        owner: SubjectId::new(owner),
        name: format!("profile-{id}"),
        description: String::new(),
        visibility,
        forked_from: None,
        uses_count: 0,
        draft: agent_profile_content(),
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    }
}

mod controls;
mod reads;

mod always_on;
