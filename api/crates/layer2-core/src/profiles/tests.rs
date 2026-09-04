use super::*;
use crate::ids::{
    AgentProfileId, SandboxProfileId, ServiceId, SkillId, SubjectId, WorkspaceProfileId,
};

const NOW: &str = "2026-09-05T00:00:00Z";

fn agent_profile(id: &str, owner: &str, visibility: ProfileVisibility) -> AgentProfile {
    AgentProfile {
        id: AgentProfileId(id.into()),
        owner: SubjectId(owner.into()),
        name: "Research agent".into(),
        description: "A careful research assistant".into(),
        visibility,
        draft: AgentProfileContent {
            model: Some("gpt-5".into()),
            thinking_effort: ThinkingEffort::High,
            permission_level: PermissionLevel::Guarded,
            skills: vec![],
            system_prompt: "Be precise.".into(),
        },
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    }
}

fn workspace_profile(id: &str, owner: &str, agent: AgentProfileRef) -> WorkspaceProfile {
    WorkspaceProfile {
        id: WorkspaceProfileId(id.into()),
        owner: SubjectId(owner.into()),
        name: "Research workspace".into(),
        description: String::new(),
        visibility: ProfileVisibility::Private,
        draft: WorkspaceProfileContent {
            location: WorkspaceLocation::GitRepository {
                url: "https://github.com/the-omega-institute/trureturing".into(),
                reference: None,
            },
            name: "trureturing".into(),
            setup_commands: vec!["lake build".into()],
            agent_profiles: vec![agent],
        },
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    }
}

fn sandbox_profile(id: &str, owner: &str, workspace: WorkspaceProfileRef) -> SandboxProfile {
    SandboxProfile {
        id: SandboxProfileId(id.into()),
        owner: SubjectId(owner.into()),
        name: "Lean research".into(),
        description: String::new(),
        visibility: ProfileVisibility::Public,
        draft: SandboxProfileContent {
            cpu_millis: 2_000,
            memory_mib: 4_096,
            nyxid_services: vec![ServiceId::new("ornn-api")],
            agent_provider: AgentProvider::Codex,
            model_access: ModelAccess::ChronoaiManaged,
            max_workspaces: 4,
            max_agents_per_workspace: 4,
            workspace_profiles: vec![workspace],
            software_install_commands: vec!["curl -fsSL https://example.invalid/lean | sh".into()],
            environment_variables: vec![EnvironmentVariable {
                name: "LEAN_VERSION".into(),
                value: "4.20.0".into(),
            }],
        },
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    }
}

fn bounds() -> ProfileCapacityBounds {
    ProfileCapacityBounds {
        min_cpu_millis: 100,
        max_cpu_millis: 8_000,
        min_memory_mib: 128,
        max_memory_mib: 16_384,
    }
}

#[test]
fn canonical_agent_digest_is_stable() {
    let content = agent_profile("agp_one", "owner", ProfileVisibility::Private).draft;
    assert_eq!(
        content_digest(&content).unwrap(),
        "5e2334daa625e6e5585e25b300ddc8cd822259ac89105909f71acc34eda2c569"
    );
}

#[test]
fn validation_refuses_reserved_environment_names_and_secret_like_values() {
    let mut profile = sandbox_profile(
        "sbp_one",
        "owner",
        WorkspaceProfileRef::new(WorkspaceProfileId("wkp_one".into()), 1),
    );
    profile.draft.environment_variables[0].name = "CHRONO_TOKEN".into();
    assert_eq!(
        profile.draft.validate(bounds()).unwrap_err().code,
        "reserved_environment_variable"
    );

    profile.draft.environment_variables[0] = EnvironmentVariable {
        name: "PUBLIC_SETTING".into(),
        value: "contains-private-marker".into(),
    };
    let error = profile
        .draft
        .validate_public_values(&["private-marker".into()])
        .unwrap_err();
    assert_eq!(error.code, "secret_like_environment_value");
}

#[test]
fn all_profile_content_refuses_unknown_fields() {
    let json =
        serde_json::to_value(agent_profile("agp_one", "owner", ProfileVisibility::Private).draft)
            .unwrap();
    let mut object = json.as_object().unwrap().clone();
    object.insert("future".into(), serde_json::json!(true));
    assert!(serde_json::from_value::<AgentProfileContent>(object.into()).is_err());

    let skill = SkillRef {
        source: SkillSource::Ornn,
        id: SkillId::new("proof-search"),
        version: Some("1".into()),
    };
    assert_eq!(
        serde_json::from_str::<SkillRef>(&serde_json::to_string(&skill).unwrap()).unwrap(),
        skill
    );
}

#[tokio::test]
async fn revisions_are_snapshots_and_later_draft_edits_do_not_change_them() {
    let store = InMemoryProfileStore::default();
    let mut profile = agent_profile("agp_one", "owner", ProfileVisibility::Private);
    store.create_agent_profile(profile.clone()).await.unwrap();
    let revision = store
        .publish_agent_profile(&profile.id, 1, NOW.into())
        .await
        .unwrap();
    let snapshot = AgentProfileSnapshot::from(&revision);

    profile.draft.system_prompt = "Changed later".into();
    profile.draft_version = 2;
    store.save_agent_draft(1, profile).await.unwrap();
    let stored = store
        .agent_profile_revision(&snapshot.id, snapshot.revision)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(AgentProfileSnapshot::from(&stored), snapshot);
    assert_eq!(snapshot.content.system_prompt, "Be precise.");
}

#[tokio::test]
async fn references_must_resolve_to_own_or_public_revisions() {
    let store = InMemoryProfileStore::default();
    let agent = agent_profile("agp_public", "publisher", ProfileVisibility::Public);
    store.create_agent_profile(agent.clone()).await.unwrap();
    store
        .publish_agent_profile(&agent.id, 1, NOW.into())
        .await
        .unwrap();

    let workspace = workspace_profile(
        "wkp_consumer",
        "consumer",
        AgentProfileRef::new(agent.id.clone(), 1),
    );
    store
        .create_workspace_profile(workspace.clone())
        .await
        .unwrap();
    store
        .publish_workspace_profile(&workspace.id, 1, NOW.into())
        .await
        .unwrap();

    let private_agent = agent_profile("agp_private", "publisher", ProfileVisibility::Private);
    store
        .create_agent_profile(private_agent.clone())
        .await
        .unwrap();
    store
        .publish_agent_profile(&private_agent.id, 1, NOW.into())
        .await
        .unwrap();
    let inaccessible = workspace_profile(
        "wkp_bad",
        "consumer",
        AgentProfileRef::new(private_agent.id, 1),
    );
    store
        .create_workspace_profile(inaccessible.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .publish_workspace_profile(&inaccessible.id, 1, NOW.into())
            .await
            .unwrap_err(),
        ProfileStoreError::ReferenceUnavailable
    );
}

#[tokio::test]
async fn sandbox_publication_resolves_workspace_and_checks_secrets() {
    let store = InMemoryProfileStore::default();
    let owner = "owner";
    let agent = agent_profile("agp_one", owner, ProfileVisibility::Private);
    store.create_agent_profile(agent.clone()).await.unwrap();
    store
        .publish_agent_profile(&agent.id, 1, NOW.into())
        .await
        .unwrap();
    let workspace = workspace_profile("wkp_one", owner, AgentProfileRef::new(agent.id.clone(), 1));
    store
        .create_workspace_profile(workspace.clone())
        .await
        .unwrap();
    store
        .publish_workspace_profile(&workspace.id, 1, NOW.into())
        .await
        .unwrap();
    let mut sandbox = sandbox_profile("sbp_one", owner, WorkspaceProfileRef::new(workspace.id, 1));
    sandbox.draft.environment_variables[0].value = "credential-shape".into();
    store.create_sandbox_profile(sandbox.clone()).await.unwrap();
    assert_eq!(
        store
            .publish_sandbox_profile(
                &sandbox.id,
                1,
                NOW.into(),
                bounds(),
                &["credential-shape".into()],
            )
            .await
            .unwrap_err(),
        ProfileStoreError::Validation(ProfileError::new(
            "secret_like_environment_value",
            "environment_variables"
        ))
    );
}
