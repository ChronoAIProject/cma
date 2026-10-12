use std::sync::atomic::{AtomicU64, Ordering};

use futures_util::future::join_all;
use layer2_core::ids::{AgentProfileId, SandboxProfileId, SubjectId, WorkspaceProfileId};
use layer2_core::profiles::store::{
    AgentProfileStore, ProfilePageRequest, ProfileStoreError, ProfileUseRecorder,
    SandboxProfileStore, WorkspaceProfileStore,
};
use layer2_core::profiles::{
    AgentProfile, AgentProfileContent, AgentProfileRef, AgentProvider, EnvironmentVariable,
    ForkedProfile, MarketplaceError, MarketplaceKind, MarketplaceLocator, MarketplaceQuery,
    MarketplaceRevision, MarketplaceStore, ModelAccess, PermissionLevel, ProfileCapacityBounds,
    ProfileVisibility, SandboxProfile, SandboxProfileContent, ThinkingEffort, WorkspaceLocation,
    WorkspaceProfile, WorkspaceProfileContent, WorkspaceProfileRef,
};
use mongodb::bson::doc;
use mongodb::Client;

use super::sandbox_mongo::MongoProfileStore;

const T1: &str = "2026-09-05T01:00:00Z";
const T2: &str = "2026-09-05T02:00:00Z";
const T3: &str = "2026-09-05T03:00:00Z";
const T4: &str = "2026-09-05T04:00:00Z";
static DATABASE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn marketplace_resolves_public_author_names_without_changing_ownership() {
    use layer2_core::{PublicIdentity, UserDirectoryStore};

    let (client, name, store) = database("authors").await;
    let directory = super::directory::MongoUserDirectoryStore::open(&client, &name)
        .await
        .unwrap();
    let profile = agent_profile("agp_author", "publisher", "Named author's profile");
    store.create_agent_profile(profile.clone()).await.unwrap();
    store
        .publish_agent_profile(&profile.id, 1, T1.into())
        .await
        .unwrap();
    let locator = MarketplaceLocator::Agent(profile.id.clone());
    make_public(&store, &profile.owner, &locator).await;
    assert_eq!(
        store
            .read_marketplace(&locator)
            .await
            .unwrap()
            .summary
            .owner_display,
        "Author unavailable"
    );
    for display_name in [
        Some("Research author"),
        Some("Renamed author"),
        Some(" "),
        None,
    ] {
        directory
            .observe(&PublicIdentity {
                subject: profile.owner.clone(),
                display_name: display_name.map(str::to_string),
                avatar_url: None,
            })
            .await
            .unwrap();
        let expected = display_name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("Author unavailable");
        let page = store
            .list_marketplace(&MarketplaceQuery::default())
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].owner_display, expected);
        let entry = store.read_marketplace(&locator).await.unwrap();
        assert_eq!(entry.summary.owner_display, expected);
        assert_eq!(entry.owner, profile.owner);
        assert_eq!(entry.summary.name, profile.name);
        let names = directory
            .display_names(&[profile.owner.clone(), SubjectId::new("missing-person")])
            .await
            .unwrap();
        assert_eq!(
            names.get(profile.owner.as_str()).map(String::as_str),
            display_name.filter(|name| !name.trim().is_empty())
        );
        assert!(!names.contains_key("missing-person"));
    }
    let names = super::directory::public_display_names(&client, &name, &["unrelated".into()])
        .await
        .unwrap();
    assert!(names.is_empty());
    assert!(
        super::directory::public_display_names(&client, &name, &vec!["publisher".into(); 201])
            .await
            .is_err()
    );
    client.database(&name).drop().await.unwrap();
}

#[path = "profile_reference_mongo_tests.rs"]
mod reference_tests;

async fn database(prefix: &str) -> (Client, String, MongoProfileStore) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(uri)
        .await
        .expect("test Mongo connects");
    let sequence = DATABASE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = format!("cma_813_{prefix}_{}_{sequence}", std::process::id());
    let store = MongoProfileStore::open(&client, &name).await.unwrap();
    (client, name, store)
}

fn bounds() -> ProfileCapacityBounds {
    ProfileCapacityBounds {
        min_cpu_millis: 100,
        max_cpu_millis: 8_000,
        min_memory_mib: 128,
        max_memory_mib: 16_384,
    }
}

fn agent_profile(id: &str, owner: &str, name: &str) -> AgentProfile {
    AgentProfile {
        id: AgentProfileId(id.into()),
        owner: SubjectId(owner.into()),
        name: name.into(),
        description: "A precision research assistant".into(),
        visibility: ProfileVisibility::Private,
        forked_from: None,
        uses_count: 0,
        draft: AgentProfileContent {
            isolated_agent: false,
            model: Some("gpt-5".into()),
            thinking_effort: ThinkingEffort::High,
            permission_level: PermissionLevel::Guarded,
            skills: vec![],
            system_prompt: "first revision".into(),
        },
        draft_version: 1,
        created_at: T1.into(),
        updated_at: T1.into(),
        retired_at: None,
    }
}

fn workspace_profile(id: &str, agent: AgentProfileRef) -> WorkspaceProfile {
    WorkspaceProfile {
        id: WorkspaceProfileId(id.into()),
        owner: SubjectId("publisher".into()),
        name: "Beta workspace".into(),
        description: "Workspace for formal research".into(),
        visibility: ProfileVisibility::Private,
        forked_from: None,
        uses_count: 0,
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
        created_at: T1.into(),
        updated_at: T1.into(),
        retired_at: None,
    }
}

fn sandbox_profile(id: &str, workspaces: Vec<WorkspaceProfileRef>) -> SandboxProfile {
    SandboxProfile {
        availability: Default::default(),
        id: SandboxProfileId(id.into()),
        owner: SubjectId("publisher".into()),
        name: "Gamma sandbox".into(),
        description: "Isolated research environment".into(),
        visibility: ProfileVisibility::Private,
        forked_from: None,
        uses_count: 0,
        draft: SandboxProfileContent {
            cpu_millis: 2_000,
            memory_mib: 4_096,
            nyxid_services: vec![],
            agent_provider: AgentProvider::Codex,
            model_access: ModelAccess::ChronoaiManaged,
            max_workspaces: 4,
            max_agents_per_workspace: 4,
            workspace_profiles: workspaces,
            software_install_commands: vec!["apt-get update".into()],
            environment_variables: vec![EnvironmentVariable {
                name: "TOOL_VERSION".into(),
                value: "1.0".into(),
            }],
        },
        draft_version: 1,
        created_at: T1.into(),
        updated_at: T1.into(),
        retired_at: None,
    }
}

async fn make_public(store: &MongoProfileStore, owner: &SubjectId, locator: &MarketplaceLocator) {
    store
        .set_profile_visibility(
            owner,
            locator,
            ProfileVisibility::Public,
            T4.into(),
            bounds(),
            &[],
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn marketplace_workflows_are_durable_and_cross_kind() {
    let (client, name, store) = database("workflow").await;
    let mut agent = agent_profile("agp_market", "publisher", "Alpha agent");
    let first_content = agent.draft.clone();
    store.create_agent_profile(agent.clone()).await.unwrap();
    let first_revision = store
        .publish_agent_profile(&agent.id, 1, T1.into())
        .await
        .unwrap();
    agent.draft.system_prompt = "second revision".into();
    agent.draft_version = 2;
    agent.updated_at = T2.into();
    store.save_agent_draft(1, agent.clone()).await.unwrap();
    store
        .publish_agent_profile(&agent.id, 2, T2.into())
        .await
        .unwrap();
    make_public(
        &store,
        &agent.owner,
        &MarketplaceLocator::Agent(agent.id.clone()),
    )
    .await;

    let workspace = workspace_profile("wkp_market", AgentProfileRef::new(agent.id.clone(), 1));
    store
        .create_workspace_profile(workspace.clone())
        .await
        .unwrap();
    let workspace_revision = store
        .publish_workspace_profile(&workspace.id, 1, T3.into())
        .await
        .unwrap();
    make_public(
        &store,
        &workspace.owner,
        &MarketplaceLocator::Workspace(workspace.id.clone()),
    )
    .await;

    let sandbox = sandbox_profile(
        "sbp_market",
        vec![WorkspaceProfileRef::new(workspace.id.clone(), 1)],
    );
    store.create_sandbox_profile(sandbox.clone()).await.unwrap();
    store
        .publish_sandbox_profile(&sandbox.id, 1, T4.into(), bounds(), &[])
        .await
        .unwrap();
    make_public(
        &store,
        &sandbox.owner,
        &MarketplaceLocator::Sandbox(sandbox.id.clone()),
    )
    .await;

    let first = store
        .list_marketplace(&MarketplaceQuery {
            limit: 1,
            ..MarketplaceQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(first.items[0].id, sandbox.id.as_str());
    let second = store
        .list_marketplace(&MarketplaceQuery {
            cursor: first.next,
            limit: 1,
            ..MarketplaceQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(second.items[0].id, workspace.id.as_str());
    let search = store
        .list_marketplace(&MarketplaceQuery {
            kind: Some(MarketplaceKind::Agent),
            text: Some("precision".into()),
            ..MarketplaceQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(search.items.len(), 1);
    assert_eq!(search.items[0].latest_revision, 2);

    let workspace_entry = store
        .read_marketplace(&MarketplaceLocator::Workspace(workspace.id.clone()))
        .await
        .unwrap();
    assert_eq!(workspace_entry.resolved_profiles[0].name, agent.name);
    let MarketplaceRevision::Agent(resolved_agent) =
        &workspace_entry.resolved_revisions[0].revision
    else {
        panic!("workspace resolved another profile kind");
    };
    assert_eq!(resolved_agent, &first_revision);
    let sandbox_entry = store
        .read_marketplace(&MarketplaceLocator::Sandbox(sandbox.id.clone()))
        .await
        .unwrap();
    let MarketplaceRevision::Workspace(resolved_workspace) =
        &sandbox_entry.resolved_revisions[0].revision
    else {
        panic!("sandbox resolved another profile kind");
    };
    assert_eq!(resolved_workspace, &workspace_revision);
    let forked = store
        .fork_profile(
            &SubjectId("consumer".into()),
            &MarketplaceLocator::Agent(agent.id.clone()),
            1,
            T4.into(),
        )
        .await
        .unwrap();
    let ForkedProfile::Agent(forked) = forked else {
        panic!("agent fork returned another profile kind");
    };
    assert_eq!(forked.draft, first_content);
    assert_eq!(
        forked.forked_from,
        Some(AgentProfileRef::new(agent.id.clone(), 1))
    );
    assert_eq!(forked.visibility, ProfileVisibility::Private);

    let locator = MarketplaceLocator::Agent(agent.id.clone());
    join_all((0..32).map(|_| store.record_profile_use(&locator)))
        .await
        .into_iter()
        .for_each(|result| result.unwrap());
    let counted = store
        .read_marketplace(&locator)
        .await
        .unwrap()
        .summary
        .uses_count;
    assert_eq!(counted, 32);
    store
        .agent_profiles
        .update_one(
            doc! { "_id": agent.id.as_str() },
            doc! { "$set": { "uses_count": i64::MAX, "record.uses_count": i64::MAX } },
        )
        .await
        .unwrap();
    store.record_profile_use(&locator).await.unwrap();
    assert_eq!(
        store
            .read_marketplace(&locator)
            .await
            .unwrap()
            .summary
            .uses_count,
        i64::MAX as u64
    );
    store
        .agent_profiles
        .update_one(
            doc! { "_id": agent.id.as_str() },
            doc! { "$set": { "record.uses_count": i64::MAX - 1 } },
        )
        .await
        .unwrap();
    assert!(matches!(
        store.record_profile_use(&locator).await,
        Err(ProfileStoreError::Corrupt(_))
    ));
    store
        .agent_profiles
        .update_one(
            doc! { "_id": agent.id.as_str() },
            doc! { "$set": { "record.uses_count": i64::MAX } },
        )
        .await
        .unwrap();

    store
        .retire_agent_profile(&agent.id, &agent.owner, T4.into())
        .await
        .unwrap();
    assert!(store
        .list_marketplace(&MarketplaceQuery {
            kind: Some(MarketplaceKind::Agent),
            ..MarketplaceQuery::default()
        })
        .await
        .unwrap()
        .items
        .is_empty());
    assert_eq!(
        store.read_marketplace(&locator).await,
        Err(MarketplaceError::NotFound)
    );
    assert_eq!(
        store
            .fork_profile(&SubjectId("consumer".into()), &locator, 1, T4.into())
            .await,
        Err(MarketplaceError::NotFound)
    );
    store.record_profile_use(&locator).await.unwrap();
    assert!(store
        .agent_profile_revision(&agent.id, 1)
        .await
        .unwrap()
        .is_some());
    let resolved_after_retirement = store
        .read_marketplace(&MarketplaceLocator::Workspace(workspace.id.clone()))
        .await
        .unwrap();
    assert_eq!(
        resolved_after_retirement.resolved_profiles[0].name,
        agent.name
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn marketplace_visibility_and_draft_controls_are_enforced() {
    let (client, name, store) = database("controls").await;
    let original = agent_profile("agp_controls", "publisher", "Controlled agent");
    let mut wrong_version = original.clone();
    wrong_version.id = AgentProfileId("agp_wrong_version".into());
    wrong_version.draft_version = 2;
    assert_eq!(
        store.create_agent_profile(wrong_version).await,
        Err(ProfileStoreError::Conflict)
    );
    store.create_agent_profile(original.clone()).await.unwrap();

    let mut candidates = Vec::new();
    let mut candidate = original.clone();
    candidate.owner = SubjectId("other".into());
    candidates.push(candidate);
    let mut candidate = original.clone();
    candidate.visibility = ProfileVisibility::Public;
    candidates.push(candidate);
    let mut candidate = original.clone();
    candidate.forked_from = Some(AgentProfileRef::new(AgentProfileId("agp_source".into()), 1));
    candidates.push(candidate);
    let mut candidate = original.clone();
    candidate.uses_count = 1;
    candidates.push(candidate);
    let mut candidate = original.clone();
    candidate.created_at = T2.into();
    candidates.push(candidate);
    let mut candidate = original.clone();
    candidate.retired_at = Some(T2.into());
    candidates.push(candidate);
    for mut candidate in candidates {
        candidate.draft_version = 2;
        assert_eq!(
            store.save_agent_draft(1, candidate).await,
            Err(ProfileStoreError::Conflict)
        );
    }

    let mut edited = original.clone();
    edited.name = "Edited agent".into();
    edited.description = "Edited description".into();
    edited.draft.system_prompt = "edited draft".into();
    edited.updated_at = T2.into();
    edited.draft_version = 2;
    store.save_agent_draft(1, edited.clone()).await.unwrap();
    assert_eq!(
        store
            .list_agent_profiles(Some(&edited.owner), false, &ProfilePageRequest::default())
            .await
            .unwrap()
            .items,
        vec![edited.clone()]
    );
    assert_eq!(
        store
            .set_profile_visibility(
                &edited.owner,
                &MarketplaceLocator::Agent(edited.id.clone()),
                ProfileVisibility::Public,
                T2.into(),
                bounds(),
                &[],
            )
            .await,
        Err(MarketplaceError::Unpublished)
    );
    store
        .retire_agent_profile(&edited.id, &edited.owner, T3.into())
        .await
        .unwrap();
    edited.retired_at = Some(T3.into());
    edited.updated_at = T4.into();
    edited.draft_version = 3;
    assert_eq!(
        store.save_agent_draft(2, edited).await,
        Err(ProfileStoreError::Conflict)
    );
    assert_eq!(
        store.save_agent_draft(u64::MAX, original.clone()).await,
        Err(ProfileStoreError::Conflict)
    );

    let system = agent_profile("agp_system", "system", "System agent");
    store.create_agent_profile(system.clone()).await.unwrap();
    assert_eq!(
        store
            .set_profile_visibility(
                &system.owner,
                &MarketplaceLocator::Agent(system.id),
                ProfileVisibility::Private,
                T2.into(),
                bounds(),
                &[],
            )
            .await,
        Err(MarketplaceError::Forbidden)
    );

    let mut sandbox = sandbox_profile("sbp_validation", vec![]);
    store.create_sandbox_profile(sandbox.clone()).await.unwrap();
    store
        .publish_sandbox_profile(&sandbox.id, 1, T1.into(), bounds(), &[])
        .await
        .unwrap();
    sandbox.draft.environment_variables[0].value = "contains-SECRET".into();
    sandbox.draft_version = 2;
    sandbox.updated_at = T2.into();
    store.save_sandbox_draft(1, sandbox.clone()).await.unwrap();
    store
        .publish_sandbox_profile(&sandbox.id, 2, T2.into(), bounds(), &[])
        .await
        .unwrap();
    let sandbox_locator = MarketplaceLocator::Sandbox(sandbox.id.clone());
    assert!(matches!(
        store
            .set_profile_visibility(
                &sandbox.owner,
                &sandbox_locator,
                ProfileVisibility::Public,
                T3.into(),
                bounds(),
                &["SECRET".into()],
            )
            .await,
        Err(MarketplaceError::PublicValues(_))
    ));
    store
        .set_profile_visibility(
            &sandbox.owner,
            &sandbox_locator,
            ProfileVisibility::Private,
            T3.into(),
            bounds(),
            &["SECRET".into()],
        )
        .await
        .unwrap();
    client.database(&name).drop().await.unwrap();
}
