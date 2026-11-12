use super::*;

pub(super) fn sandbox(id: &SandboxId, owner: SubjectId, stamp: &str) -> Sandbox {
    Sandbox {
        id: id.clone(),
        owner,
        name: "research".into(),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId::generate(),
            revision: 1,
            digest: "a".repeat(64),
            always_on: None,
            content: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1_000,
                memory_mib: 2_048,
                nyxid_services: vec![ServiceId::new("ornn-api")],
                agent_provider: AgentProvider::Codex,
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 1,
                max_agents_per_workspace: 8,
                workspace_profiles: Vec::new(),
                software_install_commands: Vec::new(),
                environment_variables: Vec::new(),
            },
        },
        status: SandboxStatus::Active,
        failure: None,
        runtime: None,
        credential: SandboxCredentialState {
            source: Default::default(),
            service_grants: Vec::new(),
            service_scope: None,
            reaches: vec![ServiceId::new("ornn-api")],
            key_generation: 3,
            delivered: true,
            current: Some(SandboxCredentialAuthority {
                generation: 3,
                provider_key_id: "provider-key-3".into(),
                provider_state_version: 3,
                issued_at: stamp.into(),
                expires_at: stamp.into(),
                grant_fingerprint: "b".repeat(64),
            }),
            pending: None,
            revoke_debt: Vec::new(),
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::None, stamp.into()),
        limits: SandboxLimits {
            max_workspaces: 1,
            max_agents_per_workspace: 8,
        },
        lifecycle: Default::default(),
        last_activity_at: stamp.into(),
        created_at: stamp.into(),
        updated_at: stamp.into(),
        retired_at: None,
        purge_after: None,
        revision: 1,
    }
}

pub(super) fn workspace(
    id: WorkspaceId,
    sandbox_id: SandboxId,
    owner: SubjectId,
    path: String,
    resolved_commit: String,
    stamp: &str,
) -> Workspace {
    Workspace {
        id,
        sandbox_id,
        owner,
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId::generate(),
            revision: 1,
            digest: "c".repeat(64),
            content: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory {
                    name: "project".into(),
                },
                name: "project".into(),
                setup_commands: Vec::new(),
                agent_profiles: Vec::new(),
            },
        },
        name: "project".into(),
        path,
        source: WorkspaceSource::Empty,
        resolved_commit: Some(resolved_commit),
        status: WorkspaceStatus::Ready,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, stamp.into()),
        next_worktree_number: 1,
        created_at: stamp.into(),
        ready_at: Some(stamp.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    }
}

pub(super) fn sandbox_spec(sandbox: &Sandbox) -> SandboxSpec {
    SandboxSpec {
        home_volume: sandbox
            .lifecycle
            .home
            .as_ref()
            .and_then(|home| home.attachment())
            .map(Box::new),
        sandbox_id: sandbox.id.to_string(),
        capacity: SandboxCapacity {
            cpu_millis: sandbox.profile.content.cpu_millis,
            memory_mib: sandbox.profile.content.memory_mib,
        },
        environment: BTreeMap::new(),
        reaches: sandbox
            .credential
            .reaches
            .iter()
            .map(ToString::to_string)
            .collect(),
        own_credential: false,
        created_by: Some(DEPLOYMENT_ID.into()),
        ..SandboxSpec::default()
    }
}

pub(super) fn agent_profile(skills: Vec<SkillRef>) -> AgentProfileSnapshot {
    AgentProfileSnapshot {
        id: AgentProfileId::generate(),
        revision: 1,
        digest: "d".repeat(64),
        content: AgentProfileContent {
            isolated_agent: false,
            model: Some("gpt-test".into()),
            thinking_effort: ThinkingEffort::High,
            permission_level: PermissionLevel::Autonomous,
            skills,
            system_prompt: "Be exact.".into(),
        },
    }
}
