//! Disposable retained resource hierarchy for credential custody tests.

use layer2_core::profiles::{
    AgentProfileContent, AgentProfileSnapshot, AgentProvider, ModelAccess, PermissionLevel,
    SandboxProfileContent, SandboxProfileSnapshot, ThinkingEffort, WorkspaceLocation,
    WorkspaceProfileContent, WorkspaceProfileSnapshot,
};
use layer2_core::{
    AgentId, AgentProfileId, RuntimeCapabilities, RuntimeCapability, SandboxId, SandboxProfileId,
    SubjectId, WorkspaceId, WorkspaceProfileId, WorktreeId,
};

use layer2_core::sandbox::*;

const NOW: &str = "2026-09-05T00:00:00Z";

pub(crate) fn sandbox(id: &str, owner: &str) -> Sandbox {
    Sandbox {
        id: SandboxId(id.into()),
        owner: SubjectId::new(owner),
        name: id.into(),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId(format!("profile-{id}")),
            revision: 1,
            digest: "a".repeat(64),
            always_on: None,
            content: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1_000,
                memory_mib: 2_048,
                nyxid_services: vec![],
                agent_provider: AgentProvider::default(),
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 16,
                max_agents_per_workspace: 16,
                workspace_profiles: vec![],
                software_install_commands: vec![],
                environment_variables: vec![],
            },
        },
        status: SandboxStatus::Active,
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
            max_workspaces: 16,
            max_agents_per_workspace: 16,
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

pub(crate) fn workspace(id: &str, sandbox_id: &str, owner: &str) -> Workspace {
    Workspace {
        id: WorkspaceId(id.into()),
        sandbox_id: SandboxId(sandbox_id.into()),
        owner: SubjectId::new(owner),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId(format!("profile-{id}")),
            revision: 1,
            digest: "b".repeat(64),
            content: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory { name: id.into() },
                name: id.into(),
                setup_commands: vec![],
                agent_profiles: vec![],
            },
        },
        name: id.into(),
        path: format!("/home/cma/{id}"),
        source: WorkspaceSource::Empty,
        resolved_commit: Some("c".repeat(40)),
        status: WorkspaceStatus::Ready,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into()),
        next_worktree_number: 1,
        created_at: NOW.into(),
        ready_at: Some(NOW.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    }
}

pub(crate) fn worktree(
    id: &str,
    workspace_id: &str,
    sandbox_id: &str,
    agent_id: &str,
    owner: &str,
) -> Worktree {
    Worktree {
        id: WorktreeId(id.into()),
        workspace_id: WorkspaceId(workspace_id.into()),
        sandbox_id: SandboxId(sandbox_id.into()),
        owner: SubjectId::new(owner),
        name: format!("wt-{id}"),
        path: format!("/home/cma/{workspace_id}/.worktrees/{id}"),
        branch: format!("cma/{id}"),
        base_commit: "c".repeat(40),
        status: WorktreeStatus::Ready,
        agent_id: Some(AgentId(agent_id.into())),
        revision: 1,
    }
}

pub(crate) fn agent(
    id: &str,
    worktree_id: &str,
    workspace_id: &str,
    sandbox_id: &str,
    owner: &str,
    status: AgentStatus,
) -> Agent {
    Agent {
        id: AgentId(id.into()),
        worktree_id: WorktreeId(worktree_id.into()),
        workspace_id: WorkspaceId(workspace_id.into()),
        sandbox_id: SandboxId(sandbox_id.into()),
        owner: SubjectId::new(owner),
        creation_key: format!("manual:{id}"),
        creation_fingerprint: "d".repeat(64),
        profile: AgentProfileSnapshot {
            id: AgentProfileId(format!("profile-{id}")),
            revision: 1,
            digest: "e".repeat(64),
            content: AgentProfileContent {
                isolated_agent: false,
                model: Some("model-a".into()),
                thinking_effort: ThinkingEffort::High,
                permission_level: PermissionLevel::Guarded,
                skills: vec![],
                system_prompt: String::new(),
            },
        },
        capabilities: RuntimeCapabilities::new([
            RuntimeCapability::Conversation,
            RuntimeCapability::Usage,
            RuntimeCapability::Resources,
        ]),
        status,
        conversation: ConversationState {
            turn_timeout: None,
            preferences: Default::default(),
            managed_guidance_revision: 0,
            observations: Default::default(),
            settings: None,
            conversation_id: None,
            runtime_generation: None,
            last_seq: 1,
            first_message: None,
            active_turn: None,
            operation: None,
            queued_messages: vec![],
        },
        work: AgentWorkState::new(AgentWorkStep::None, NOW.into()),
        failure: None,
        title: None,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        ready_at: None,
        stopped_at: None,
        requested_stop_reason: None,
        stop_reason: None,
        deleted_at: None,
        origin: AgentOrigin::Manual,
        revision: 1,
    }
}
