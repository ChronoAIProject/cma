use layer2_core::ids::*;
use layer2_core::profiles::*;
use layer2_core::sandbox::store::AgentAdmission;
use layer2_core::sandbox::*;
pub(crate) const NOW: &str = "2026-09-05T00:00:00Z";

pub(crate) fn sandbox(suffix: &str) -> Sandbox {
    Sandbox {
        id: SandboxId(format!("sbx_admission_{suffix}")),
        owner: SubjectId("owner".into()),
        name: format!("admission-{suffix}"),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId("sbp_admission".into()),
            revision: 1,
            digest: "a".repeat(64),
            content: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1_000,
                memory_mib: 2_048,
                nyxid_services: vec![],
                agent_provider: AgentProvider::Codex,
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 4,
                max_agents_per_workspace: 4,
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
            key_generation: 1,
            delivered: true,
            current: Some(SandboxCredentialAuthority {
                generation: 1,
                provider_key_id: "provider-key".into(),
                provider_state_version: 1,
                issued_at: NOW.into(),
                expires_at: layer2_core::rfc3339_at(
                    std::time::SystemTime::now() + std::time::Duration::from_secs(3600),
                ),
                grant_fingerprint: "a".repeat(64),
            }),
            pending: None,
            revoke_debt: vec![],
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::WaitForCredentialRotation, NOW.into()),
        limits: SandboxLimits {
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

pub(crate) fn workspace(sandbox: &Sandbox, suffix: &str) -> Workspace {
    let name = format!("workspace-{suffix}");
    Workspace {
        id: WorkspaceId(format!("wks_admission_{suffix}")),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId("wkp_admission".into()),
            revision: 1,
            digest: "b".repeat(64),
            content: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory { name: name.clone() },
                name: name.clone(),
                setup_commands: vec![],
                agent_profiles: vec![],
            },
        },
        name: name.clone(),
        path: format!("/home/cma/{name}"),
        source: WorkspaceSource::Empty,
        resolved_commit: Some("a".repeat(40)),
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

pub(crate) fn admission(workspace: &Workspace, suffix: &str) -> AgentAdmission {
    AgentAdmission {
        binding: None,
        created_by: None,
        owner: workspace.owner.clone(),
        workspace_id: workspace.id.clone(),
        agent_id: AgentId(format!("agt_admission_{suffix}")),
        worktree_id: WorktreeId(format!("wkt_admission_{suffix}")),
        profile: AgentProfileSnapshot {
            id: AgentProfileId("agp_admission".into()),
            revision: 1,
            digest: "c".repeat(64),
            content: AgentProfileContent {
                isolated_agent: false,
                model: None,
                thinking_effort: ThinkingEffort::Medium,
                permission_level: PermissionLevel::Guarded,
                skills: vec![],
                system_prompt: String::new(),
            },
        },
        capabilities: layer2_core::RuntimeCapabilities::new([
            layer2_core::RuntimeCapability::Conversation,
        ]),
        title: None,
        first_message: None,
        origin: AgentOrigin::Manual,
        creation_key: format!("admission-{suffix}"),
        event_at: NOW.into(),
    }
}
