use super::*;
use layer2_core::profiles::store::SandboxProfileStore;

pub(super) fn bounds() -> ProfileCapacityBounds {
    ProfileCapacityBounds {
        min_cpu_millis: 100,
        max_cpu_millis: 8_000,
        min_memory_mib: 128,
        max_memory_mib: 32_768,
    }
}

pub(super) async fn seed_workspace_profile(
    profiles: &InMemoryProfileStore,
    index: u16,
) -> WorkspaceProfileRef {
    let id = WorkspaceProfileId(format!("wkp_worker_{index}"));
    profiles
        .create_workspace_profile(WorkspaceProfile {
            internal: false,
            id: id.clone(),
            owner: SubjectId::new("owner"),
            name: format!("Workspace {index}"),
            description: "Worker integration fixture".into(),
            visibility: ProfileVisibility::Private,
            forked_from: None,
            uses_count: 0,
            draft: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory {
                    name: format!("project-{index}"),
                },
                name: format!("project-{index}"),
                setup_commands: Vec::new(),
                agent_profiles: Vec::new(),
            },
            draft_version: 1,
            created_at: PROFILE_TIME.into(),
            updated_at: PROFILE_TIME.into(),
            retired_at: None,
        })
        .await
        .unwrap();
    let revision = profiles
        .publish_workspace_profile(&id, 1, PROFILE_TIME.into())
        .await
        .unwrap();
    WorkspaceProfileRef::new(id, revision.revision)
}

pub(super) async fn seed_sandbox_profile(
    profiles: &InMemoryProfileStore,
    commands: Vec<String>,
    workspace_profiles: Vec<WorkspaceProfileRef>,
) -> SandboxProfileRef {
    seed_sandbox_profile_with_capacity(
        profiles,
        SandboxProfileId("sbp_worker".into()),
        commands,
        workspace_profiles,
        layer3_adapter::SandboxCapacity {
            cpu_millis: 1_000,
            memory_mib: 2_048,
        },
        None,
    )
    .await
}

pub(crate) async fn seed_sandbox_profile_with_capacity(
    profiles: &dyn SandboxProfileStore,
    id: SandboxProfileId,
    commands: Vec<String>,
    workspace_profiles: Vec<WorkspaceProfileRef>,
    capacity: layer3_adapter::SandboxCapacity,
    runtime_image: Option<String>,
) -> SandboxProfileRef {
    profiles
        .create_sandbox_profile(SandboxProfile {
            internal: false,
            availability: Default::default(),
            id: id.clone(),
            owner: SubjectId::new("owner"),
            name: "Worker sandbox".into(),
            description: "Worker integration fixture".into(),
            visibility: ProfileVisibility::Private,
            forked_from: None,
            uses_count: 0,
            draft: SandboxProfileContent {
                runtime_image,
                cpu_millis: capacity.cpu_millis,
                memory_mib: capacity.memory_mib,
                nyxid_services: vec![ServiceId::new("profile-service")],
                agent_provider: AgentProvider::Codex,
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 4,
                max_agents_per_workspace: 4,
                workspace_profiles,
                software_install_commands: commands,
                environment_variables: vec![EnvironmentVariable {
                    name: "PROJECT_MODE".into(),
                    value: "integration".into(),
                }],
            },
            draft_version: 1,
            created_at: PROFILE_TIME.into(),
            updated_at: PROFILE_TIME.into(),
            retired_at: None,
        })
        .await
        .unwrap();
    let revision = profiles
        .publish_sandbox_profile(&id, 1, PROFILE_TIME.into(), bounds(), &[])
        .await
        .unwrap();
    SandboxProfileRef::new(id, revision.revision)
}

pub(super) fn caller(subject: &str, capabilities: impl IntoIterator<Item = Capability>) -> Caller {
    Caller::new(SubjectId::new(subject), Authorization::new(capabilities))
}

pub(super) fn start_time(sandbox: &Sandbox) -> SystemTime {
    SystemTime::UNIX_EPOCH
        + Duration::from_secs(seconds_from_rfc3339(&sandbox.created_at).unwrap() + 1)
}

pub(super) fn stamp_time(stamp: &str) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds_from_rfc3339(stamp).unwrap())
}
