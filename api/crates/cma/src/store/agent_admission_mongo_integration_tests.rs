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
        checkpoint: None,
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
        owner: workspace.owner.clone(),
        workspace_id: workspace.id.clone(),
        agent_id: AgentId(format!("agt_admission_{suffix}")),
        worktree_id: WorktreeId(format!("wkt_admission_{suffix}")),
        profile: AgentProfileSnapshot {
            id: AgentProfileId("agp_admission".into()),
            revision: 1,
            digest: "c".repeat(64),
            content: AgentProfileContent {
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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn admission_replays_reject_an_incomplete_log() {
    let (client, database, store, _, _, admission) = fixture("replay_orphan").await;
    let creation = match store.admit_agent(admission.clone()).await.unwrap() {
        AgentAdmissionResult::Created(creation) => creation,
        AgentAdmissionResult::Existing(_) => panic!("fresh admission unexpectedly replayed"),
    };
    store
        .agent_log
        .insert_one(log_document(&orphan(&admission, 2)).unwrap())
        .await
        .unwrap();

    assert_eq!(
        store.admit_agent(admission.clone()).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    let mut anchor_session = store.client.start_session().await.unwrap();
    store
        .agents
        .find_one(doc! { "_id": admission.agent_id.as_str() })
        .session(&mut anchor_session)
        .await
        .unwrap();
    let snapshot_time = anchor_session.operation_time().unwrap();
    assert_eq!(
        replay_after_duplicate(&store, &admission, snapshot_time).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(
        store.get_agent(&admission.agent_id).await.unwrap(),
        Some(creation.agent)
    );

    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn admission_reconciliation_rejects_an_above_high_water_orphan() {
    let (client, database, store, _, _, admission) = fixture("reconcile_orphan").await;
    let mut transaction = store.client.start_session().await.unwrap();
    start_transaction(&mut transaction).await.unwrap();
    let receipt = match super::apply_admission(&store, &admission, &mut transaction).await {
        Ok(AdmissionReceipt::Created(receipt)) => receipt,
        Ok(AdmissionReceipt::Existing(_)) => panic!("fresh admission unexpectedly replayed"),
        Err(_) => panic!("fresh admission transaction failed"),
    };
    transaction.commit_transaction().await.unwrap();

    let mut anchor_session = store.client.start_session().await.unwrap();
    store
        .agent_log
        .insert_one(log_document(&orphan(&admission, 2)).unwrap())
        .session(&mut anchor_session)
        .await
        .unwrap();
    let snapshot_time = anchor_session.operation_time().unwrap();
    assert_eq!(
        reconcile_created(&store, &receipt, snapshot_time).await,
        Err(SandboxStoreError::SequenceConflict)
    );

    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn admission_replay_majority_commits_and_retries_an_ambiguous_commit() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
        .expect("CHRONO_TEST_MONGODB_FAILPOINT_URI must name a failpoint-enabled replica set");
    let suffix = format!("replay_commit_{}", uuid::Uuid::new_v4().simple());
    let app_name = format!("cma-agent-admission-{suffix}");
    let database = format!("cma_811_adm_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let commands = Arc::new(Mutex::new(Vec::<(String, Document)>::new()));
    let observed_commands = Arc::clone(&commands);
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            observed_commands
                .lock()
                .unwrap()
                .push((event.command_name, event.command));
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let parent = sandbox(&suffix);
    store.create_sandbox(parent.clone()).await.unwrap();
    let workspace = workspace(&parent, &suffix);
    store.create_workspace(workspace.clone()).await.unwrap();
    let admission = admission(&workspace, &suffix);
    let created = match store.admit_agent(admission.clone()).await.unwrap() {
        AgentAdmissionResult::Created(creation) => creation,
        AgentAdmissionResult::Existing(_) => panic!("fresh admission unexpectedly replayed"),
    };
    commands.lock().unwrap().clear();

    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand",
            "mode": { "times": 5_i32 },
            "data": {
                "failCommands": ["commitTransaction"],
                "appName": &app_name,
                "writeConcernError": {
                    "code": 50_i32,
                    "codeName": "MaxTimeMSExpired",
                    "errmsg": "injected ambiguous replay commit",
                },
            },
        })
        .await
        .unwrap();
    let outcome = store.admit_agent(admission).await;
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .expect("commit failpoint must be disabled before assertions");

    assert_eq!(outcome.unwrap(), AgentAdmissionResult::Existing(created));
    let commands = commands.lock().unwrap().clone();
    let commits = commands
        .iter()
        .filter_map(|(name, command)| (name == "commitTransaction").then_some(command))
        .collect::<Vec<_>>();
    assert_eq!(commits.len(), 6);
    assert!(commits.iter().all(|command| {
        command
            .get_document("writeConcern")
            .ok()
            .and_then(|concern| concern.get_str("w").ok())
            == Some("majority")
    }));
    let first_transaction = commits[0].get("txnNumber").unwrap();
    assert!(commits[..5]
        .iter()
        .all(|command| command.get("txnNumber") == Some(first_transaction)));
    assert_ne!(commits[5].get("txnNumber"), Some(first_transaction));
    assert!(!commands.iter().any(|(name, _)| name == "abortTransaction"));

    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn admission_reconciles_every_artifact_at_the_ambiguous_commit_time() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
        .expect("CHRONO_TEST_MONGODB_FAILPOINT_URI must name a failpoint-enabled replica set");
    let suffix = format!("ambiguous_{}", uuid::Uuid::new_v4().simple());
    let app_name = format!("cma-agent-admission-{suffix}");
    let database = format!("cma_811_adm_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let commands = Arc::new(Mutex::new(Vec::<(String, Document)>::new()));
    let observed_commands = Arc::clone(&commands);
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            observed_commands
                .lock()
                .unwrap()
                .push((event.command_name, event.command));
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let parent = sandbox(&suffix);
    store.create_sandbox(parent.clone()).await.unwrap();
    let workspace = workspace(&parent, &suffix);
    store.create_workspace(workspace.clone()).await.unwrap();
    let admission = admission(&workspace, &suffix);
    commands.lock().unwrap().clear();

    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand",
            "mode": { "times": 5_i32 },
            "data": {
                "failCommands": ["commitTransaction"],
                "appName": &app_name,
                "writeConcernError": {
                    "code": 50_i32,
                    "codeName": "MaxTimeMSExpired",
                    "errmsg": "injected ambiguous commit",
                },
            },
        })
        .await
        .unwrap();
    let outcome = store.admit_agent(admission.clone()).await;
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .expect("commit failpoint must be disabled before assertions");

    let creation = match outcome.unwrap() {
        AgentAdmissionResult::Created(creation) => creation,
        AgentAdmissionResult::Existing(_) => panic!("fresh admission unexpectedly replayed"),
    };
    let commands = commands.lock().unwrap().clone();
    let commit_positions = commands
        .iter()
        .enumerate()
        .filter_map(|(position, (name, _))| (name == "commitTransaction").then_some(position))
        .collect::<Vec<_>>();
    assert_eq!(commit_positions.len(), 5);
    let reconciliation_finds = commands
        .iter()
        .skip(commit_positions.last().copied().unwrap() + 1)
        .filter_map(|(name, command)| (name == "find").then_some(command))
        .collect::<Vec<_>>();
    assert_eq!(
        reconciliation_finds
            .iter()
            .map(|command| command.get_str("find").unwrap())
            .collect::<Vec<_>>(),
        vec![
            "workspaces",
            "sandboxes",
            "worktrees",
            "agents",
            "agent_log",
            "sandbox_events",
            "sandbox_events",
        ]
    );
    let snapshot_time = reconciliation_finds[0]
        .get_document("readConcern")
        .unwrap()
        .get_timestamp("atClusterTime")
        .unwrap();
    assert!(reconciliation_finds.iter().all(|command| {
        command
            .get_document("readConcern")
            .ok()
            .is_some_and(|concern| {
                concern.get_str("level").ok() == Some("snapshot")
                    && concern.get_timestamp("atClusterTime").ok() == Some(snapshot_time)
            })
    }));
    assert_eq!(
        store.get_agent(&creation.agent.id).await.unwrap(),
        Some(creation.agent)
    );
    assert_eq!(
        store.get_worktree(&creation.worktree.id).await.unwrap(),
        Some(creation.worktree)
    );

    client.database(&database).drop().await.unwrap();
}
