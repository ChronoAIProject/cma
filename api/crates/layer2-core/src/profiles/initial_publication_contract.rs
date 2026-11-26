//! Shared memory/Mongo behavior contract for deterministic private creation.
use super::*;
use crate::ids::{AgentProfileId, SandboxProfileId, SubjectId, WorkspaceProfileId};

const NOW: &str = "2026-11-16T00:00:00Z";

pub async fn verify<S>(first: &S, second: &S)
where
    S: AgentProfileStore + WorkspaceProfileStore + SandboxProfileStore,
{
    let owner = SubjectId::new("initial-profile-owner");
    let agent = AgentProfile {
        internal: true,
        id: AgentProfileId::generate(),
        owner: owner.clone(),
        name: "Initial agent".into(),
        description: String::new(),
        visibility: ProfileVisibility::Private,
        forked_from: None,
        uses_count: 0,
        draft: AgentProfileContent {
            isolated_agent: false,
            model: None,
            thinking_effort: ThinkingEffort::High,
            permission_level: PermissionLevel::Guarded,
            skills: vec![],
            system_prompt: "Initial content".into(),
        },
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    };
    first.create_agent_profile(agent.clone()).await.unwrap();
    // Both callers begin with the same unpublished draft; each may enter on a
    // different AM/store instance. The retry must not consume revision two.
    assert!(first
        .agent_profile_revision(&agent.id, 1)
        .await
        .unwrap()
        .is_none());
    assert!(second
        .agent_profile_revision(&agent.id, 1)
        .await
        .unwrap()
        .is_none());
    let (a, b) = tokio::join!(
        first.ensure_initial_agent_profile(&agent, NOW.into()),
        second.ensure_initial_agent_profile(&agent, "2026-11-16T00:00:01Z".into()),
    );
    let a = a.unwrap();
    assert_eq!(b.unwrap(), a);
    assert_eq!(a.revision, 1);
    // A client that lost the committed reply retries with a different local
    // timestamp, but still obtains the exact original immutable receipt.
    assert_eq!(
        second
            .ensure_initial_agent_profile(&agent, "2026-11-16T01:00:00Z".into())
            .await
            .unwrap(),
        a
    );
    assert_eq!(
        first
            .list_agent_profile_revisions(&agent.id, &ProfilePageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    let mut wrong = agent.clone();
    wrong.owner = SubjectId::new("another-owner");
    assert_eq!(
        second
            .ensure_initial_agent_profile(&wrong, NOW.into())
            .await,
        Err(ProfileStoreError::Conflict)
    );
    wrong = agent.clone();
    wrong.draft.system_prompt = "Different intent".into();
    assert_eq!(
        second
            .ensure_initial_agent_profile(&wrong, NOW.into())
            .await,
        Err(ProfileStoreError::Conflict)
    );

    let workspace = WorkspaceProfile {
        internal: true,
        id: WorkspaceProfileId::generate(),
        owner: owner.clone(),
        name: "Initial workspace".into(),
        description: String::new(),
        visibility: ProfileVisibility::Private,
        forked_from: None,
        uses_count: 0,
        draft: WorkspaceProfileContent {
            location: WorkspaceLocation::Directory {
                name: "workspace".into(),
            },
            name: "workspace".into(),
            setup_commands: vec![],
            agent_profiles: vec![AgentProfileRef::new(agent.id.clone(), 1)],
        },
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    };
    first
        .create_workspace_profile(workspace.clone())
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        first.ensure_initial_workspace_profile(&workspace, NOW.into()),
        second.ensure_initial_workspace_profile(&workspace, "2026-11-16T00:00:01Z".into()),
    );
    let a = a.unwrap();
    assert_eq!(b.unwrap(), a);
    assert_eq!(a.revision, 1);
    assert_eq!(
        second
            .ensure_initial_workspace_profile(&workspace, "2026-11-16T01:00:00Z".into())
            .await
            .unwrap(),
        a
    );
    assert_eq!(
        first
            .list_workspace_profile_revisions(&workspace.id, &ProfilePageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    let mut wrong = workspace.clone();
    wrong.owner = SubjectId::new("another-owner");
    assert_eq!(
        second
            .ensure_initial_workspace_profile(&wrong, NOW.into())
            .await,
        Err(ProfileStoreError::Conflict)
    );
    wrong = workspace.clone();
    wrong.draft.setup_commands.push("echo changed".into());
    assert_eq!(
        second
            .ensure_initial_workspace_profile(&wrong, NOW.into())
            .await,
        Err(ProfileStoreError::Conflict)
    );

    let bounds = ProfileCapacityBounds {
        min_cpu_millis: 1,
        max_cpu_millis: 4000,
        min_memory_mib: 1,
        max_memory_mib: 8192,
    };
    let scanner: [String; 0] = [];
    let sandbox = SandboxProfile {
        internal: true,
        availability: Default::default(),
        id: SandboxProfileId::generate(),
        owner,
        name: "Initial sandbox".into(),
        description: String::new(),
        visibility: ProfileVisibility::Private,
        forked_from: None,
        uses_count: 0,
        draft: SandboxProfileContent {
            runtime_image: None,
            cpu_millis: 1000,
            memory_mib: 1024,
            nyxid_services: vec![],
            agent_provider: AgentProvider::default(),
            model_access: ModelAccess::ChronoaiManaged,
            max_workspaces: 4,
            max_agents_per_workspace: 4,
            workspace_profiles: vec![WorkspaceProfileRef::new(workspace.id.clone(), 1)],
            software_install_commands: vec![],
            environment_variables: vec![],
        },
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    };
    first.create_sandbox_profile(sandbox.clone()).await.unwrap();
    let (a, b) = tokio::join!(
        first.ensure_initial_sandbox_profile(&sandbox, NOW.into(), bounds, &scanner),
        second.ensure_initial_sandbox_profile(
            &sandbox,
            "2026-11-16T00:00:01Z".into(),
            bounds,
            &scanner
        ),
    );
    let a = a.unwrap();
    assert_eq!(b.unwrap(), a);
    assert_eq!(a.revision, 1);
    assert_eq!(
        second
            .ensure_initial_sandbox_profile(
                &sandbox,
                "2026-11-16T01:00:00Z".into(),
                bounds,
                &scanner
            )
            .await
            .unwrap(),
        a
    );
    assert_eq!(
        first
            .list_sandbox_profile_revisions(&sandbox.id, &ProfilePageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    let mut wrong = sandbox.clone();
    wrong.owner = SubjectId::new("another-owner");
    assert_eq!(
        second
            .ensure_initial_sandbox_profile(&wrong, NOW.into(), bounds, &scanner)
            .await,
        Err(ProfileStoreError::Conflict)
    );
    wrong = sandbox.clone();
    wrong.draft.memory_mib += 1;
    assert_eq!(
        second
            .ensure_initial_sandbox_profile(&wrong, NOW.into(), bounds, &scanner)
            .await,
        Err(ProfileStoreError::Conflict)
    );

    // Ordinary explicit publication retains its existing revision semantics.
    assert_eq!(
        first
            .publish_agent_profile(&agent.id, 1, NOW.into())
            .await
            .unwrap()
            .revision,
        2
    );
    assert_eq!(
        first
            .publish_workspace_profile(&workspace.id, 1, NOW.into())
            .await
            .unwrap()
            .revision,
        2
    );
    assert_eq!(
        first
            .publish_sandbox_profile(&sandbox.id, 1, NOW.into(), bounds, &scanner)
            .await
            .unwrap()
            .revision,
        2
    );
    assert_eq!(
        second
            .ensure_initial_agent_profile(&agent, NOW.into())
            .await
            .unwrap()
            .revision,
        1
    );
    assert_eq!(
        second
            .ensure_initial_workspace_profile(&workspace, NOW.into())
            .await
            .unwrap()
            .revision,
        1
    );
    assert_eq!(
        second
            .ensure_initial_sandbox_profile(&sandbox, NOW.into(), bounds, &scanner)
            .await
            .unwrap()
            .revision,
        1
    );
}

/// Exercise the production deterministic helper as well as the store primitive.
pub async fn verify_creation<S>(first: std::sync::Arc<S>, second: std::sync::Arc<S>)
where
    S: crate::bots::BotProfileStore + 'static,
{
    let bounds = ProfileCapacityBounds {
        min_cpu_millis: 1,
        max_cpu_millis: 4000,
        min_memory_mib: 1,
        max_memory_mib: 8192,
    };
    let scanner = std::sync::Arc::new([] as [String; 0]);
    let a = PrivateProfileCreationService::new(first.clone(), bounds, scanner.clone());
    let b = PrivateProfileCreationService::new(second, bounds, scanner);
    let owner = SubjectId::new("initial-helper-owner");
    let key = uuid::Uuid::new_v4().to_string();
    let content = AgentProfileContent {
        isolated_agent: false,
        model: None,
        thinking_effort: ThinkingEffort::High,
        permission_level: PermissionLevel::Guarded,
        skills: vec![],
        system_prompt: "Concurrent helper".into(),
    };
    let (left, right) = tokio::join!(
        a.create_agent_for_owner(&owner, &key, "Agent".into(), String::new(), content.clone()),
        b.create_agent_for_owner(&owner, &key, "Agent".into(), String::new(), content),
    );
    let agent = left.unwrap();
    assert_eq!(right.unwrap(), agent);
    assert_eq!(
        first
            .list_agent_profile_revisions(&agent.profile_id, &ProfilePageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    let content = WorkspaceProfileContent {
        location: WorkspaceLocation::Directory {
            name: "workspace".into(),
        },
        name: "workspace".into(),
        setup_commands: vec![],
        agent_profiles: vec![AgentProfileRef::new(agent.profile_id, 1)],
    };
    let (left, right) = tokio::join!(
        a.create_workspace_for_owner(
            &owner,
            &key,
            "Workspace".into(),
            String::new(),
            content.clone()
        ),
        b.create_workspace_for_owner(
            &owner,
            &key,
            "Workspace".into(),
            String::new(),
            content.clone()
        ),
    );
    let left = retry_unavailable(
        left,
        a.create_workspace_for_owner(
            &owner,
            &key,
            "Workspace".into(),
            String::new(),
            content.clone(),
        ),
    )
    .await;
    let right = retry_unavailable(
        right,
        b.create_workspace_for_owner(&owner, &key, "Workspace".into(), String::new(), content),
    )
    .await;
    let workspace = left.unwrap();
    assert_eq!(right.unwrap(), workspace);
    assert_eq!(
        first
            .list_workspace_profile_revisions(&workspace.profile_id, &ProfilePageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    let content = SandboxProfileContent {
        runtime_image: None,
        cpu_millis: 1000,
        memory_mib: 1024,
        nyxid_services: vec![],
        agent_provider: AgentProvider::default(),
        model_access: ModelAccess::ChronoaiManaged,
        max_workspaces: 4,
        max_agents_per_workspace: 4,
        workspace_profiles: vec![WorkspaceProfileRef::new(workspace.profile_id, 1)],
        software_install_commands: vec![],
        environment_variables: vec![],
    };
    let (left, right) = tokio::join!(
        a.create_sandbox_for_owner(
            &owner,
            &key,
            "Sandbox".into(),
            String::new(),
            content.clone()
        ),
        b.create_sandbox_for_owner(
            &owner,
            &key,
            "Sandbox".into(),
            String::new(),
            content.clone()
        ),
    );
    let left = retry_unavailable(
        left,
        a.create_sandbox_for_owner(
            &owner,
            &key,
            "Sandbox".into(),
            String::new(),
            content.clone(),
        ),
    )
    .await;
    let right = retry_unavailable(
        right,
        b.create_sandbox_for_owner(&owner, &key, "Sandbox".into(), String::new(), content),
    )
    .await;
    let sandbox = left.unwrap();
    assert_eq!(right.unwrap(), sandbox);
    assert_eq!(
        first
            .list_sandbox_profile_revisions(&sandbox.profile_id, &ProfilePageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}

/// A transactional draft insert that loses a write conflict to its concurrent
/// twin reports bounded unavailability and never replays itself. The caller's
/// ordinary retry with the same creation key then returns the twin's creation.
async fn retry_unavailable<T>(
    result: Result<T, ProfileCreationError>,
    retry: impl std::future::Future<Output = Result<T, ProfileCreationError>>,
) -> Result<T, ProfileCreationError> {
    match result {
        Err(ProfileCreationError::Store(ProfileStoreError::Unavailable(_))) => retry.await,
        other => other,
    }
}

#[cfg(test)]
#[tokio::test]
async fn initial_publication_memory_preserves_one_revision_and_explicit_publish() {
    let first = InMemoryProfileStore::default();
    verify(&first, &first.clone()).await;
    verify_creation(
        std::sync::Arc::new(first.clone()),
        std::sync::Arc::new(first),
    )
    .await;
}
