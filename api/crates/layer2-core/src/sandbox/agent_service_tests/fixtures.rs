use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::profiles::store::{AgentProfileStore, InMemoryProfileStore};
use crate::profiles::{
    AgentProfile, AgentProfileContent, AgentProfileRef, AgentProvider, ModelAccess,
    PermissionLevel, ProfileVisibility, SandboxProfileContent, SandboxProfileSnapshot,
    ThinkingEffort, WorkspaceLocation, WorkspaceProfileContent, WorkspaceProfileSnapshot,
};
use crate::sandbox::store::{InMemorySandboxStore, SandboxStore, WorkspaceStore};
use crate::{
    AgentId, AgentProfileId, Authorization, Caller, Capability, RuntimeCapabilities, SandboxId,
    SandboxProfileId, SubjectId, WorkspaceId, WorkspaceProfileId,
};
use layer3_adapter::ConversationInputSender;

use super::super::*;

pub(super) const NOW: &str = "2026-09-05T00:00:00Z";

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

pub(super) fn profile(id: &str, owner: &str, visibility: ProfileVisibility) -> AgentProfile {
    AgentProfile {
        id: AgentProfileId(id.into()),
        owner: SubjectId::new(owner),
        name: "Research agent".into(),
        description: "Checks a repository".into(),
        visibility,
        forked_from: None,
        uses_count: 0,
        draft: AgentProfileContent {
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
                cpu_millis: 1_000,
                memory_mib: 2_048,
                nyxid_services: vec![],
                agent_provider: AgentProvider::Codex,
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
        checkpoint: None,
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

pub(super) async fn uses(profiles: &InMemoryProfileStore, id: &AgentProfileId) -> u64 {
    profiles
        .agent_profile(id)
        .await
        .unwrap()
        .unwrap()
        .uses_count
}
