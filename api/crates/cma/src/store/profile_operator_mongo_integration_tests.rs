use std::sync::atomic::{AtomicU64, Ordering};

use layer2_core::ids::{AgentProfileId, SandboxProfileId, SubjectId, WorkspaceProfileId};
use layer2_core::profiles::store::{
    AgentProfileStore, ProfilePageRequest, ProfileStoreError, SandboxProfileStore,
    WorkspaceProfileStore,
};
use layer2_core::profiles::{
    AgentProfile, AgentProfileContent, AgentProvider, ModelAccess, PermissionLevel,
    ProfileAdminQuery, ProfileAdminStore, ProfileKind, ProfileLocator, ProfileVisibility,
    SandboxProfile, SandboxProfileContent, ThinkingEffort, WorkspaceLocation, WorkspaceProfile,
    WorkspaceProfileContent,
};
use mongodb::bson::{doc, Document};
use mongodb::Client;

use super::sandbox_mongo::{MongoProfileStore, AGENT_PROFILES, DOCUMENT_SCHEMA_VERSION};

const T1: &str = "2026-09-05T01:00:00Z";
const T2: &str = "2026-09-05T02:00:00Z";
const T3: &str = "2026-09-05T03:00:00Z";
static DATABASE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

async fn database(prefix: &str) -> (Client, String, MongoProfileStore) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(uri)
        .await
        .expect("test Mongo connects");
    let sequence = DATABASE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = format!("cma_814_profile_{prefix}_{}_{sequence}", std::process::id());
    let store = MongoProfileStore::open(&client, &name).await.unwrap();
    (client, name, store)
}

fn agent_profile(id: &str, owner: &str, visibility: ProfileVisibility) -> AgentProfile {
    AgentProfile {
        id: AgentProfileId(id.into()),
        owner: SubjectId(owner.into()),
        name: format!("Agent {id}"),
        description: String::new(),
        visibility,
        forked_from: None,
        uses_count: 0,
        draft: AgentProfileContent {
            isolated_agent: false,
            model: None,
            thinking_effort: ThinkingEffort::Medium,
            permission_level: PermissionLevel::Guarded,
            skills: vec![],
            system_prompt: String::new(),
        },
        draft_version: 1,
        created_at: T1.into(),
        updated_at: T1.into(),
        retired_at: None,
    }
}

fn workspace_profile(id: &str, owner: &str, visibility: ProfileVisibility) -> WorkspaceProfile {
    WorkspaceProfile {
        id: WorkspaceProfileId(id.into()),
        owner: SubjectId(owner.into()),
        name: format!("Workspace {id}"),
        description: String::new(),
        visibility,
        forked_from: None,
        uses_count: 0,
        draft: WorkspaceProfileContent {
            location: WorkspaceLocation::Directory {
                name: "project".into(),
            },
            name: "project".into(),
            setup_commands: vec![],
            agent_profiles: vec![],
        },
        draft_version: 1,
        created_at: T1.into(),
        updated_at: T1.into(),
        retired_at: None,
    }
}

fn sandbox_profile(id: &str, owner: &str, visibility: ProfileVisibility) -> SandboxProfile {
    SandboxProfile {
        availability: Default::default(),
        id: SandboxProfileId(id.into()),
        owner: SubjectId(owner.into()),
        name: format!("Sandbox {id}"),
        description: String::new(),
        visibility,
        forked_from: None,
        uses_count: 0,
        draft: SandboxProfileContent {
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
        draft_version: 1,
        created_at: T1.into(),
        updated_at: T1.into(),
        retired_at: None,
    }
}

fn query(limit: u16, cursor: Option<String>) -> ProfileAdminQuery {
    ProfileAdminQuery {
        page: ProfilePageRequest { limit, cursor },
        ..ProfileAdminQuery::default()
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn profile_inventory_pages_globally_and_applies_each_exact_filter() {
    let (client, name, store) = database("inventory").await;
    let agent_a = agent_profile("agp_admin_a", "owner-a", ProfileVisibility::Private);
    let agent_z = agent_profile("agp_admin_z", "owner-b", ProfileVisibility::Public);
    let sandbox = sandbox_profile("sbp_admin_m", "owner-a", ProfileVisibility::Public);
    let workspace = workspace_profile("wkp_admin_n", "owner-a", ProfileVisibility::Private);
    store.create_agent_profile(agent_a.clone()).await.unwrap();
    store.create_agent_profile(agent_z.clone()).await.unwrap();
    store.create_sandbox_profile(sandbox.clone()).await.unwrap();
    store
        .create_workspace_profile(workspace.clone())
        .await
        .unwrap();

    let page = store.list_profiles(&query(2, None)).await.unwrap();
    assert_eq!(
        page.items
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["agp_admin_a", "agp_admin_z"]
    );
    assert_eq!(page.next.as_deref(), Some("agp_admin_z"));
    let page = store.list_profiles(&query(2, page.next)).await.unwrap();
    assert_eq!(
        page.items
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["sbp_admin_m", "wkp_admin_n"]
    );
    assert_eq!(page.next, None);

    let mut filtered = query(10, None);
    filtered.owner = Some(SubjectId("owner-a".into()));
    assert_eq!(store.list_profiles(&filtered).await.unwrap().items.len(), 3);
    filtered.owner = None;
    filtered.visibility = Some(ProfileVisibility::Public);
    assert_eq!(store.list_profiles(&filtered).await.unwrap().items.len(), 2);
    filtered.visibility = None;
    filtered.kind = Some(ProfileKind::Sandbox);
    let rows = store.list_profiles(&filtered).await.unwrap().items;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, sandbox.id.as_str());
    assert_eq!(rows[0].kind, ProfileKind::Sandbox);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn profile_retirement_is_cross_kind_owner_independent_idempotent_and_schema_guarded() {
    let (client, name, store) = database("retirement").await;
    let agent = agent_profile("agp_retire", "another-owner", ProfileVisibility::Private);
    let workspace = workspace_profile("wkp_retire", "another-owner", ProfileVisibility::Private);
    let sandbox = sandbox_profile("sbp_retire", "another-owner", ProfileVisibility::Private);
    store.create_agent_profile(agent.clone()).await.unwrap();
    store
        .create_workspace_profile(workspace.clone())
        .await
        .unwrap();
    store.create_sandbox_profile(sandbox.clone()).await.unwrap();

    for locator in [
        ProfileLocator::Agent(AgentProfileId(layer2_core::DEFAULT_AGENT_PROFILE_ID.into())),
        ProfileLocator::Workspace(WorkspaceProfileId(
            layer2_core::DEFAULT_WORKSPACE_PROFILE_ID.into(),
        )),
        ProfileLocator::Sandbox(SandboxProfileId(
            layer2_core::DEFAULT_SANDBOX_PROFILE_ID.into(),
        )),
    ] {
        assert_eq!(
            store.retire_profile(&locator, T2.into()).await,
            Err(ProfileStoreError::Protected)
        );
    }

    for locator in [
        ProfileLocator::Agent(agent.id.clone()),
        ProfileLocator::Workspace(workspace.id.clone()),
        ProfileLocator::Sandbox(sandbox.id.clone()),
    ] {
        let retired = store.retire_profile(&locator, T2.into()).await.unwrap();
        assert_eq!(retired.owner.as_str(), "another-owner");
        assert_eq!(retired.updated_at, T2);
        assert_eq!(retired.retired_at.as_deref(), Some(T2));
        let repeated = store.retire_profile(&locator, T3.into()).await.unwrap();
        assert_eq!(repeated.updated_at, T2);
        assert_eq!(repeated.retired_at.as_deref(), Some(T2));
    }

    let future_id = "agp_future";
    client
        .database(&name)
        .collection::<Document>(AGENT_PROFILES)
        .insert_one(doc! {
            "_id": future_id,
            "document_schema_version": DOCUMENT_SCHEMA_VERSION + 1,
            "retired_at": null,
        })
        .await
        .unwrap();
    assert!(matches!(
        store
            .retire_profile(
                &ProfileLocator::Agent(AgentProfileId(future_id.into())),
                T2.into(),
            )
            .await,
        Err(ProfileStoreError::Corrupt(_))
    ));
    assert_eq!(
        store
            .retire_profile(
                &ProfileLocator::Agent(AgentProfileId("agp_missing".into())),
                T2.into(),
            )
            .await,
        Err(ProfileStoreError::NotFound)
    );
    client.database(&name).drop().await.unwrap();
}
