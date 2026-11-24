use std::sync::{Arc, Mutex};

use layer2_core::ids::{
    AgentId, AgentProfileId, SandboxId, SandboxProfileId, SubjectId, WorkspaceId,
    WorkspaceProfileId, WorktreeId,
};
use layer2_core::profiles::{
    AgentProfileContent, AgentProfileSnapshot, AgentProvider, ModelAccess, PermissionLevel,
    SandboxProfileContent, SandboxProfileSnapshot, ThinkingEffort, WorkspaceLocation,
    WorkspaceProfileContent, WorkspaceProfileSnapshot,
};
use layer2_core::sandbox::store::{
    AgentAdmission, AgentAdmissionResult, AgentAdmissionStore, AgentStore, SandboxStore,
    SandboxStoreError, WorkspaceStore, WorktreeStore,
};
use layer2_core::sandbox::{
    AgentLogEntry, AgentLogKind, AgentOrigin, Sandbox, SandboxCredentialAuthority,
    SandboxCredentialState, SandboxLimits, SandboxOrigin, SandboxSignInState, SandboxStatus,
    SandboxWorkState, SandboxWorkStep, Workspace, WorkspaceOrigin, WorkspaceSource,
    WorkspaceStatus, WorkspaceWorkState, WorkspaceWorkStep,
};
use mongodb::bson::{doc, Document};
use mongodb::event::command::CommandEvent;
use mongodb::event::EventHandler;
use mongodb::options::ClientOptions;
use mongodb::Client;

use super::super::agent_admission_records::{
    reconcile_created, replay_after_duplicate, AdmissionReceipt,
};
use super::super::agent_log::log_document;
use super::super::sandbox_mongo::{start_transaction, MongoSandboxStore};

const NOW: &str = "2026-09-05T00:00:00Z";

#[path = "agent_admission_wake_mongo_tests.rs"]
mod admission_wake;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn trigger_profile_reuse_is_atomic_and_replays_the_same_agent_after_newer_creation() {
    let (client, database, store, parent, mut workspace, mut automatic) =
        fixture("trigger_profile_reuse").await;
    workspace.profile.content.agent_profiles = vec![layer2_core::profiles::AgentProfileRef::new(
        automatic.profile.id.clone(),
        automatic.profile.revision,
    )];
    workspace.profile.digest =
        layer2_core::profiles::content_digest(&workspace.profile.content).unwrap();
    let revision = workspace.revision;
    workspace.revision += 1;
    store
        .save_workspace(revision, workspace.clone())
        .await
        .unwrap();
    automatic.origin = AgentOrigin::WorkspaceProfile { index: 0 };
    automatic.creation_key = format!("workspace-profile:{}:0", workspace.id);
    let mut trigger = automatic.clone();
    trigger.agent_id = AgentId::generate();
    trigger.worktree_id = WorktreeId::generate();
    trigger.origin = AgentOrigin::Trigger {
        trigger_id: "trg_profile_reuse".into(),
        launch_id: "lch_profile_reuse".into(),
    };
    trigger.creation_key = format!(
        "trigger-profile:{}",
        layer2_core::profiles::content_digest(&trigger.origin).unwrap()
    );
    trigger.first_message = Some(
        layer2_core::sandbox::FirstMessageRecord::new(
            trigger.creation_key.clone(),
            "Introduce the repository.",
            NOW,
        )
        .unwrap(),
    );
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..8 {
        let store = store.clone();
        let automatic = automatic.clone();
        let trigger = trigger.clone();
        tasks.spawn(async move {
            if index % 2 == 0 {
                store.admit_agent(automatic).await
            } else {
                store.admit_agent_reusing_profile(trigger).await
            }
        });
    }
    let mut ids = std::collections::BTreeSet::new();
    while let Some(result) = tasks.join_next().await {
        ids.insert(result.unwrap().unwrap().creation().agent.id.clone());
    }
    assert_eq!(ids.len(), 1);
    assert_eq!(
        store
            .agents
            .count_documents(doc! { "sandbox_id": parent.id.as_str() })
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .worktrees
            .count_documents(doc! { "sandbox_id": parent.id.as_str() })
            .await
            .unwrap(),
        1
    );
    let agent_id = ids.pop_first().unwrap();
    let first = store.get_agent(&agent_id).await.unwrap().unwrap();
    use layer2_core::sandbox::store::AgentProvenanceStore;
    let provenance = store
        .agent_provenance_snapshot(&agent_id)
        .await
        .unwrap()
        .creation_provenance(&agent_id)
        .unwrap();
    assert_eq!(provenance.provisioning_origin, first.origin);
    assert_eq!(
        first.conversation.first_message.as_ref().unwrap().text,
        "Introduce the repository."
    );
    let mut newer = trigger.clone();
    newer.agent_id = AgentId::generate();
    newer.worktree_id = WorktreeId::generate();
    newer.origin = AgentOrigin::Manual;
    newer.creation_key = "manual-newer-agent".into();
    newer.first_message = None;
    newer.event_at = "2026-09-05T00:01:00Z".into();
    store.admit_agent(newer).await.unwrap();
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    let replay = reopened
        .admit_agent_reusing_profile(trigger.clone())
        .await
        .unwrap();
    assert_eq!(replay.creation().agent.id, agent_id);
    assert_eq!(
        reopened
            .agent_provenance_snapshot(&agent_id)
            .await
            .unwrap()
            .creation
            .unwrap(),
        provenance
    );
    assert_eq!(
        replay.creation().agent.conversation.first_message,
        first.conversation.first_message
    );
    trigger.first_message = Some(
        layer2_core::sandbox::FirstMessageRecord::new(
            trigger.creation_key.clone(),
            "Changed prompt",
            NOW,
        )
        .unwrap(),
    );
    assert_eq!(
        reopened.admit_agent_reusing_profile(trigger).await,
        Err(SandboxStoreError::AgentCreationConflict)
    );
    client.database(&database).drop().await.unwrap();
}

#[path = "agent_admission_mongo_integration_tests/creation_revival.rs"]
mod creation_revival;
#[path = "agent_admission_mongo_integration_tests/ledger_maintenance.rs"]
mod ledger_maintenance;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn automatic_profile_admission_is_pinned_and_replayed_atomically() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = Client::with_uri_str(uri).await.unwrap();
    let database = format!("cma_profile_admission_{}", uuid::Uuid::new_v4().simple());
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let parent = sandbox("automatic");
    let mut workspace = workspace(&parent, "automatic");
    let mut input = admission(&workspace, "automatic");
    workspace.profile.content.agent_profiles = vec![layer2_core::profiles::AgentProfileRef::new(
        input.profile.id.clone(),
        input.profile.revision,
    )];
    workspace.profile.digest =
        layer2_core::profiles::content_digest(&workspace.profile.content).unwrap();
    store.create_sandbox(parent.clone()).await.unwrap();
    store.create_workspace(workspace.clone()).await.unwrap();
    input.origin = AgentOrigin::WorkspaceProfile { index: 0 };
    input.creation_key = format!("workspace-profile:{}:0", workspace.id);
    let mut invalid = input.clone();
    invalid.origin = AgentOrigin::WorkspaceProfile { index: 1 };
    invalid.creation_key = format!("workspace-profile:{}:1", workspace.id);
    assert!(matches!(
        store.admit_agent(invalid).await,
        Err(SandboxStoreError::InvalidTransaction(_))
    ));
    let mut attempts = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let store = store.clone();
        let mut candidate = input.clone();
        candidate.agent_id = AgentId::generate();
        candidate.worktree_id = WorktreeId::generate();
        attempts.spawn(async move { store.admit_agent(candidate).await });
    }
    let mut created = 0;
    let mut identities = std::collections::BTreeSet::new();
    while let Some(result) = attempts.join_next().await {
        let result = result.unwrap().unwrap();
        created += usize::from(matches!(result, AgentAdmissionResult::Created(_)));
        identities.insert(result.creation().agent.id.clone());
    }
    assert_eq!(created, 1);
    assert_eq!(identities.len(), 1);
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    let replay = reopened.admit_agent(input.clone()).await.unwrap();
    assert!(matches!(replay, AgentAdmissionResult::Existing(_)));
    assert!(identities.contains(&replay.creation().agent.id));
    let mut paused = reopened.get_sandbox(&parent.id).await.unwrap().unwrap();
    let revision = paused.revision;
    paused.status = SandboxStatus::Installing;
    paused.revision += 1;
    reopened.save_sandbox(revision, paused).await.unwrap();
    input.creation_key.push_str("-new");
    assert_eq!(
        reopened.admit_agent(input).await,
        Err(SandboxStoreError::SandboxNotReady)
    );
    client.database(&database).drop().await.unwrap();
}

async fn fixture(
    suffix: &str,
) -> (
    Client,
    String,
    MongoSandboxStore,
    Sandbox,
    Workspace,
    AgentAdmission,
) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(uri)
        .await
        .expect("test Mongo connects");
    let database = format!("cma_811_agent_admission_{suffix}_{}", std::process::id());
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let sandbox = sandbox(suffix);
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let workspace = workspace(&sandbox, suffix);
    store.create_workspace(workspace.clone()).await.unwrap();
    let admission = admission(&workspace, suffix);
    (client, database, store, sandbox, workspace, admission)
}

fn sandbox(suffix: &str) -> Sandbox {
    Sandbox {
        id: SandboxId(format!("sbx_admission_{suffix}")),
        owner: SubjectId("owner".into()),
        name: format!("admission-{suffix}"),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId("sbp_admission".into()),
            revision: 1,
            digest: "a".repeat(64),
            always_on: None,
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
                expires_at: "2026-10-05T00:00:00Z".into(),
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

fn workspace(sandbox: &Sandbox, suffix: &str) -> Workspace {
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

fn admission(workspace: &Workspace, suffix: &str) -> AgentAdmission {
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

fn orphan(admission: &AgentAdmission, seq: u64) -> AgentLogEntry {
    AgentLogEntry {
        agent_id: admission.agent_id.clone(),
        seq,
        appended_at: NOW.into(),
        kind: AgentLogKind::Deleted,
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn fresh_admission_rejects_a_noncolliding_orphan_log() {
    let (client, database, store, sandbox, workspace, admission) = fixture("orphan").await;
    store
        .agent_log
        .insert_one(log_document(&orphan(&admission, 2)).unwrap())
        .await
        .unwrap();

    assert_eq!(
        store.admit_agent(admission.clone()).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(sandbox));
    assert_eq!(
        store.get_workspace(&workspace.id).await.unwrap(),
        Some(workspace)
    );
    assert_eq!(store.get_agent(&admission.agent_id).await.unwrap(), None);
    assert_eq!(
        store.get_worktree(&admission.worktree_id).await.unwrap(),
        None
    );
    assert_eq!(
        store
            .agent_log
            .count_documents(doc! { "agent_id": admission.agent_id.as_str() })
            .await
            .unwrap(),
        1
    );

    client.database(&database).drop().await.unwrap();
}

#[path = "agent_admission_mongo_integration_tests/receipt_tests.rs"]
mod receipt_tests;

#[path = "agent_admission_mongo_integration_tests/channel_usage.rs"]
mod channel_usage;
#[path = "agent_admission_mongo_integration_tests/provenance.rs"]
mod provenance;

#[path = "agent_admission_mongo_integration_tests/provenance_backfill.rs"]
mod provenance_backfill;
#[path = "agent_admission_mongo_integration_tests/provenance_inventory.rs"]
mod provenance_inventory;

#[path = "agent_admission_mongo_integration_tests/contention.rs"]
mod contention;

#[path = "agent_worktree_recovery_mongo_tests.rs"]
mod worktree_recovery;

#[path = "agent_admission_mongo_integration_tests/segmented.rs"]
mod segmented;

#[path = "agent_admission_mongo_integration_tests/metadata_unknown_tests.rs"]
pub(in crate::store) mod metadata_unknown;
