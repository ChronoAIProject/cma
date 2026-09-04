use std::sync::atomic::{AtomicU64, Ordering};

use layer2_core::ids::{
    AgentId, AgentProfileId, SandboxId, SandboxProfileId, SubjectId, WorkspaceId, WorktreeId,
};
use layer2_core::profiles::store::{AgentProfileStore, ProfilePageRequest, ProfileStoreError};
use layer2_core::profiles::{
    AgentProfile, AgentProfileContent, AgentProfileSnapshot, AgentProvider, ModelAccess,
    PermissionLevel, ProfileVisibility, SandboxProfileContent, SandboxProfileSnapshot,
    ThinkingEffort,
};
use layer2_core::sandbox::store::{
    AgentLogStore, AgentStore, SandboxEventStore, SandboxStore, SandboxStoreError,
    SandboxTransaction, SandboxTransactionStore, WorktreeStore,
};
use layer2_core::sandbox::{
    Agent, AgentLogKind, AgentOrigin, AgentStatus, ConversationState, Sandbox,
    SandboxCredentialState, SandboxEvent, SandboxEventKind, SandboxLimits, SandboxStatus, Worktree,
    WorktreeStatus,
};
use mongodb::bson::{doc, Document};
use mongodb::Client;

use super::sandbox_events::event_document;
use super::sandbox_mongo::{
    MongoProfileStore, MongoSandboxStore, AGENTS, AGENT_LOG, AGENT_PROFILES,
    AGENT_PROFILE_REVISIONS, CHECKPOINTS, SANDBOXES, SANDBOX_EVENTS, SANDBOX_PROFILES,
    SANDBOX_PROFILE_REVISIONS, WORKSPACES, WORKSPACE_PROFILES, WORKSPACE_PROFILE_REVISIONS,
    WORKTREES,
};
use super::DefaultProfileSpec;

const NOW: &str = "2026-09-05T00:00:00Z";
static DATABASE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

async fn database(prefix: &str) -> (Client, String) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(uri)
        .await
        .expect("test Mongo connects");
    let sequence = DATABASE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = format!("cma_804_{prefix}_{}_{sequence}", std::process::id());
    (client, name)
}

fn sandbox(id: &str) -> Sandbox {
    Sandbox {
        id: SandboxId(id.into()),
        owner: SubjectId("owner".into()),
        name: "research".into(),
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId("sbp_test".into()),
            revision: 1,
            digest: "a".repeat(64),
            content: SandboxProfileContent {
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
            reaches: vec![],
            key_generation: 1,
            delivered: true,
        },
        limits: SandboxLimits {
            max_workspaces: 4,
            max_agents_per_workspace: 4,
        },
        checkpoint: None,
        last_activity_at: NOW.into(),
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
        revision: 1,
    }
}

fn agent_profile(id: &str) -> AgentProfile {
    AgentProfile {
        id: AgentProfileId(id.into()),
        owner: SubjectId("owner".into()),
        name: "Research agent".into(),
        description: String::new(),
        visibility: ProfileVisibility::Private,
        forked_from: None,
        uses_count: 0,
        draft: AgentProfileContent {
            model: None,
            thinking_effort: ThinkingEffort::Medium,
            permission_level: PermissionLevel::Guarded,
            skills: vec![],
            system_prompt: String::new(),
        },
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    }
}

fn worktree_and_agent(sandbox_id: &SandboxId) -> (Worktree, Agent) {
    let agent_id = AgentId("agt_atomic".into());
    let worktree = Worktree {
        id: WorktreeId("wkt_atomic".into()),
        workspace_id: WorkspaceId("wks_atomic".into()),
        sandbox_id: sandbox_id.clone(),
        owner: SubjectId("owner".into()),
        name: "wt-1".into(),
        path: "/home/cma/project/.worktrees/wt-1".into(),
        branch: "cma/wt-1".into(),
        base_commit: "0123456789abcdef".into(),
        status: WorktreeStatus::Ready,
        agent_id: Some(agent_id.clone()),
        revision: 1,
    };
    let agent = Agent {
        id: agent_id,
        worktree_id: worktree.id.clone(),
        workspace_id: worktree.workspace_id.clone(),
        sandbox_id: sandbox_id.clone(),
        owner: worktree.owner.clone(),
        profile: AgentProfileSnapshot {
            id: AgentProfileId("agp_test".into()),
            revision: 1,
            digest: "b".repeat(64),
            content: agent_profile("agp_test").draft,
        },
        status: AgentStatus::Starting,
        conversation: ConversationState {
            conversation_id: None,
            last_seq: 0,
            first_message: None,
        },
        title: None,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        ready_at: None,
        stopped_at: None,
        stop_reason: None,
        deleted_at: None,
        origin: AgentOrigin::Manual,
        revision: 1,
    };
    (worktree, agent)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn fresh_database_initializes_every_schema_v1_collection_and_index() {
    let (client, name) = database("fresh").await;
    let sandbox_store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let profile_store = MongoProfileStore::open(&client, &name).await.unwrap();
    let names = client
        .database(&name)
        .list_collection_names()
        .await
        .unwrap();
    for required in [
        SANDBOXES,
        WORKSPACES,
        WORKTREES,
        AGENTS,
        AGENT_LOG,
        SANDBOX_EVENTS,
        CHECKPOINTS,
        AGENT_PROFILES,
        AGENT_PROFILE_REVISIONS,
        WORKSPACE_PROFILES,
        WORKSPACE_PROFILE_REVISIONS,
        SANDBOX_PROFILES,
        SANDBOX_PROFILE_REVISIONS,
    ] {
        assert!(
            names.iter().any(|name| name == required),
            "missing {required}"
        );
    }
    assert!(sandbox_store
        .sandboxes
        .list_index_names()
        .await
        .unwrap()
        .contains(&"sandboxes_owner_name_live".to_string()));
    assert!(profile_store
        .agent_revisions
        .list_index_names()
        .await
        .unwrap()
        .contains(&"agent_profile_revisions_profile_revision".to_string()));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn default_profiles_seed_idempotently_and_refuse_configuration_drift() {
    let (client, name) = database("default_profiles").await;
    let store = MongoProfileStore::open(&client, &name).await.unwrap();
    let bounds = layer2_core::profiles::ProfileCapacityBounds {
        min_cpu_millis: 100,
        max_cpu_millis: 8_000,
        min_memory_mib: 128,
        max_memory_mib: 16_384,
    };
    let spec = DefaultProfileSpec {
        cpu_millis: 1_000,
        memory_mib: 1_024,
        agent_model: "model-a".into(),
        agent_effort: ThinkingEffort::High,
    };
    store
        .seed_default_profiles(&spec, bounds, &[])
        .await
        .unwrap();
    store
        .seed_default_profiles(&spec, bounds, &[])
        .await
        .unwrap();

    let mut changed = spec;
    changed.agent_model = "model-b".into();
    assert!(matches!(
        store.seed_default_profiles(&changed, bounds, &[]).await,
        Err(layer2_core::StoreError::Malformed(_))
    ));
    assert_eq!(
        store
            .agent_revisions
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn startup_refuses_an_unknown_document_schema_version() {
    let (client, name) = database("unknown_version").await;
    client
        .database(&name)
        .collection::<Document>(SANDBOXES)
        .insert_one(doc! { "_id": "future", "document_schema_version": 2_i64 })
        .await
        .unwrap();
    let error = match MongoSandboxStore::open(&client, &name).await {
        Ok(_) => panic!("an unknown schema version must stop startup"),
        Err(error) => error,
    };
    assert!(matches!(error, layer2_core::StoreError::Malformed(_)));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unique_names_and_revision_cas_return_typed_errors() {
    let (client, name) = database("cas").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let original = sandbox("sbx_cas_one");
    store.create_sandbox(original.clone()).await.unwrap();
    let mut same_id = original.clone();
    same_id.name = "must-not-replace".into();
    assert_eq!(
        store.create_sandbox(same_id).await,
        Err(SandboxStoreError::Duplicate)
    );
    assert_eq!(
        store.get_sandbox(&original.id).await.unwrap(),
        Some(original.clone())
    );
    assert_eq!(
        store.save_sandbox(u64::MAX, original.clone()).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.get_sandbox(&original.id).await.unwrap(),
        Some(original.clone())
    );
    let mut duplicate = original.clone();
    duplicate.id = SandboxId("sbx_cas_two".into());
    assert_eq!(
        store.create_sandbox(duplicate).await,
        Err(SandboxStoreError::Duplicate)
    );
    let mut replacement = original.clone();
    replacement.revision = 2;
    replacement.updated_at = "2026-09-05T00:00:01Z".into();
    store.save_sandbox(1, replacement.clone()).await.unwrap();
    replacement.updated_at = "2026-09-05T00:00:02Z".into();
    assert_eq!(
        store.save_sandbox(1, replacement).await,
        Err(SandboxStoreError::Conflict)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn injected_event_failure_rolls_back_worktree_agent_and_sequence() {
    let (client, name) = database("atomic").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let sandbox = sandbox("sbx_atomic");
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let conflicting = SandboxEvent {
        sandbox_id: sandbox.id.clone(),
        seq: 1,
        appended_at: NOW.into(),
        kind: SandboxEventKind::Created,
    };
    store
        .sandbox_events
        .insert_one(event_document(&conflicting).unwrap())
        .await
        .unwrap();
    let (worktree, agent) = worktree_and_agent(&sandbox.id);
    let error = store
        .commit(SandboxTransaction::CreateWorktreeAndAgent {
            worktree: worktree.clone(),
            agent: agent.clone(),
            event_at: NOW.into(),
            event: SandboxEventKind::AgentCreated {
                workspace_id: worktree.workspace_id.clone(),
                worktree_id: worktree.id.clone(),
                agent_id: agent.id.clone(),
            },
        })
        .await
        .unwrap_err();
    assert_eq!(error, SandboxStoreError::SequenceConflict);
    assert_eq!(store.get_worktree(&worktree.id).await.unwrap(), None);
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), None);
    assert_eq!(
        store
            .sandboxes
            .find_one(doc! { "_id": sandbox.id.0 })
            .await
            .unwrap()
            .unwrap()
            .get_i64("last_seq")
            .unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn missing_event_in_the_durable_range_fails_closed() {
    let (client, name) = database("gap").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let sandbox = sandbox("sbx_gap");
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let first = store
        .append_sandbox_event(&sandbox.id, NOW.into(), SandboxEventKind::Created)
        .await
        .unwrap();
    let second = store
        .append_sandbox_event(&sandbox.id, NOW.into(), SandboxEventKind::Created)
        .await
        .unwrap();
    assert_eq!((first.seq, second.seq), (1, 2));
    store
        .sandbox_events
        .delete_one(doc! { "sandbox_id": sandbox.id.as_str(), "seq": 1_i64 })
        .await
        .unwrap();
    assert_eq!(
        store.sandbox_events(&sandbox.id, 0, 10).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn missing_agent_log_entry_in_the_durable_range_fails_closed() {
    let (client, name) = database("agent_log_gap").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let sandbox = sandbox("sbx_agent_log_gap");
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let (_, agent) = worktree_and_agent(&sandbox.id);
    store.create_agent(agent.clone()).await.unwrap();
    let first = store
        .append_agent_log(
            &agent.id,
            NOW.into(),
            AgentLogKind::Stopped {
                reason: "first".into(),
            },
        )
        .await
        .unwrap();
    let second = store
        .append_agent_log(
            &agent.id,
            NOW.into(),
            AgentLogKind::Stopped {
                reason: "second".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!((first.seq, second.seq), (1, 2));
    assert_eq!(
        store
            .get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .conversation
            .last_seq,
        2
    );
    store
        .agent_log
        .delete_one(doc! { "agent_id": agent.id.as_str(), "seq": 1_i64 })
        .await
        .unwrap();
    assert_eq!(
        store.agent_log(&agent.id, 0, 10).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn profile_publication_is_transactional_and_revisions_page_newest_first() {
    let (client, name) = database("profiles").await;
    let store = MongoProfileStore::open(&client, &name).await.unwrap();
    let profile = agent_profile("agp_publication");
    store.create_agent_profile(profile.clone()).await.unwrap();
    assert_eq!(
        store
            .publish_agent_profile(&profile.id, 1, NOW.into())
            .await
            .unwrap()
            .revision,
        1
    );
    assert_eq!(
        store
            .publish_agent_profile(&profile.id, 1, NOW.into())
            .await
            .unwrap()
            .revision,
        2
    );
    let first = store
        .list_agent_profile_revisions(
            &profile.id,
            &ProfilePageRequest {
                limit: 1,
                cursor: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(first.items[0].revision, 2);
    assert_eq!(first.next.as_deref(), Some("2"));
    let second = store
        .list_agent_profile_revisions(
            &profile.id,
            &ProfilePageRequest {
                limit: 1,
                cursor: first.next,
            },
        )
        .await
        .unwrap();
    assert_eq!(second.items[0].revision, 1);
    let mut stale = profile;
    stale.draft_version = 2;
    assert_eq!(
        store.save_agent_draft(0, stale).await,
        Err(ProfileStoreError::Conflict)
    );
    client.database(&name).drop().await.unwrap();
}
