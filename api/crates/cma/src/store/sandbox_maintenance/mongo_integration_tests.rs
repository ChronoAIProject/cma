use std::sync::Arc;

use layer2_core::ids::{
    AgentId, AgentProfileId, SubjectId, WorkspaceId, WorkspaceProfileId, WorktreeId,
};
use layer2_core::profiles::{
    AgentProfileContent, AgentProfileSnapshot, PermissionLevel, ThinkingEffort, WorkspaceLocation,
    WorkspaceProfileContent, WorkspaceProfileSnapshot,
};
use layer2_core::sandbox::store::{
    AgentAdmission, AgentAdmissionStore, AgentConversationMutation, AgentConversationStore,
    AgentLogStore, AgentStore, SandboxEventStore, SandboxStore, WorkspaceStore,
};
use layer2_core::sandbox::{
    inactivity_pause_notice, Agent, AgentLogEntry, AgentLogKind, AgentOrigin, AgentStatus,
    AgentWorkState, AgentWorkStep, ConversationState, RuntimeBinding, Sandbox, SandboxEvent,
    SandboxEventKind, SandboxLifecycleOperation, SandboxStatus, Workspace, WorkspaceOrigin,
    WorkspaceSource, WorkspaceStatus, WorkspaceWorkState, WorkspaceWorkStep,
};
use mongodb::options::ClientOptions;
use mongodb::Client;

use super::*;
use crate::store::agent_log::log_document;
use crate::store::agents::agent_document;
use crate::store::sandbox_events::event_document;
use crate::store::sandbox_mongo::start_transaction;
use crate::store::sandbox_mongo_integration_tests::sandbox;

const IDLE_AT: &str = "2026-09-05T00:00:00Z";
const LATER: &str = "2026-09-05T00:30:00Z";

#[path = "availability_integration_tests.rs"]
mod availability;
#[path = "restored_connection_integration_tests.rs"]
mod restored_connection_integration_tests;
#[path = "runtime_update_integration_tests.rs"]
mod runtime_update_integration_tests;

pub(in crate::store) async fn database() -> (Client, String, MongoSandboxStore) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(uri).await.unwrap();
    let name = format!("cma_812_maintenance_{}", uuid::Uuid::new_v4().simple());
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    (client, name, store)
}

pub(in crate::store) fn active_sandbox(id: &str) -> Sandbox {
    let mut value = sandbox(id);
    value.name = id.into();
    value.runtime = Some(RuntimeBinding {
        boot_id: None,
        runtime_id: format!("runtime-{id}"),
        runtime_version: "image-v1".into(),
        provision_generation: Some(1),
        bound_at: IDLE_AT.into(),
    });
    value.lifecycle.runtime_generation_high_water = 1;
    value
}

pub(in crate::store) fn ready_agent(sandbox: &Sandbox, id: &str) -> Agent {
    Agent {
        id: AgentId(id.into()),
        worktree_id: WorktreeId(format!("wkt_{id}")),
        workspace_id: WorkspaceId(format!("wks_{}", sandbox.id)),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        creation_key: format!("create-{id}"),
        creation_fingerprint: "c".repeat(64),
        profile: AgentProfileSnapshot {
            id: AgentProfileId("agp_maintenance".into()),
            revision: 1,
            digest: "d".repeat(64),
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
        status: AgentStatus::Ready,
        conversation: ConversationState {
            observations: Default::default(),
            settings: None,
            conversation_id: Some(format!("conversation-{id}")),
            runtime_generation: Some(1),
            last_seq: 1,
            first_message: None,
            active_turn: None,
            operation: None,
            queued_messages: vec![],
        },
        work: AgentWorkState::new(AgentWorkStep::None, IDLE_AT.into()),
        failure: None,
        title: None,
        created_at: IDLE_AT.into(),
        updated_at: IDLE_AT.into(),
        ready_at: Some(IDLE_AT.into()),
        stopped_at: None,
        requested_stop_reason: None,
        stop_reason: None,
        deleted_at: None,
        origin: AgentOrigin::Manual,
        revision: 1,
    }
}

pub(in crate::store) async fn insert_agent(
    store: &MongoSandboxStore,
    agent: &Agent,
    appended_at: &str,
) {
    store
        .agents
        .insert_one(agent_document(agent).unwrap())
        .await
        .unwrap();
    let entry = AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: 1,
        appended_at: appended_at.into(),
        kind: AgentLogKind::ConversationOpened {
            conversation_id: agent.conversation.conversation_id.clone().unwrap(),
            runtime_generation: 1,
        },
    };
    store
        .agent_log
        .insert_one(log_document(&entry).unwrap())
        .await
        .unwrap();
}

pub(in crate::store) async fn fixture(store: &MongoSandboxStore, id: &str) -> (Sandbox, Agent) {
    let sandbox = active_sandbox(id);
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let agent = ready_agent(&sandbox, &format!("agt_{id}"));
    insert_agent(store, &agent, IDLE_AT).await;
    (sandbox, agent)
}

fn pause(id: &SandboxId) -> SandboxInactivityPauseRequest {
    SandboxInactivityPauseRequest {
        sandbox_id: id.clone(),
        requested_at: LATER.into(),
        pause_after_seconds: 900,
    }
}

fn rotation(id: &SandboxId) -> SandboxImageRotationRequest {
    SandboxImageRotationRequest {
        sandbox_id: id.clone(),
        requested_at: LATER.into(),
        expected_runtime_version: "image-v2".into(),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn maintenance_pages_globally_in_exclusive_id_order_and_rejects_corrupt_rows() {
    let (client, name, store) = database().await;
    for (id, owner, status) in [
        ("sbx_z", "owner-a", SandboxStatus::Active),
        ("sbx_a", "owner-b", SandboxStatus::Active),
        ("sbx_m", "owner-a", SandboxStatus::Active),
        ("sbx_b", "owner-c", SandboxStatus::Provisioning),
    ] {
        let mut value = active_sandbox(id);
        value.owner = SubjectId(owner.into());
        value.status = status;
        store.create_sandbox(value).await.unwrap();
    }
    let first = store
        .sandbox_maintenance_candidates(&PageRequest::new(2, None).unwrap())
        .await
        .unwrap();
    assert_eq!(
        first.items,
        [SandboxId("sbx_a".into()), SandboxId("sbx_m".into())]
    );
    assert_eq!(first.next.as_deref(), Some("sbx_m"));
    let second = store
        .sandbox_maintenance_candidates(&PageRequest::new(2, first.next).unwrap())
        .await
        .unwrap();
    assert_eq!(second.items, [SandboxId("sbx_z".into())]);
    assert_eq!(second.next, None);
    for limit in [0, MAX_PAGE_SIZE + 1] {
        assert_eq!(
            store
                .sandbox_maintenance_candidates(&PageRequest {
                    limit,
                    cursor: None
                })
                .await,
            Err(SandboxStoreError::InvalidPage)
        );
    }
    store
        .sandboxes
        .update_one(
            doc! { "_id": "sbx_a" },
            doc! { "$set": { "record.id": "sbx_other" } },
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .sandbox_maintenance_candidates(&PageRequest::default())
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn maintenance_pause_reads_every_live_agent_and_preserves_model_rows() {
    let (client, name, store) = database().await;
    let (sandbox, first) = fixture(&store, "sbx_idle").await;
    let second = ready_agent(&sandbox, "agt_second");
    insert_agent(&store, &second, LATER).await;
    assert_eq!(
        store
            .schedule_inactivity_pause(pause(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(
        store.get_sandbox(&sandbox.id).await.unwrap(),
        Some(sandbox.clone())
    );
    store
        .agent_log
        .update_one(
            doc! { "agent_id": second.id.as_str(), "seq": 1_i64 },
            doc! { "$set": { "record.appended_at": IDLE_AT } },
        )
        .await
        .unwrap();
    store
        .append_sandbox_event(&sandbox.id, IDLE_AT.into(), SandboxEventKind::Created)
        .await
        .unwrap();
    assert_eq!(
        store
            .schedule_inactivity_pause(pause(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    let paused = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
    assert_eq!(paused.status, SandboxStatus::Pausing);
    assert_eq!(paused.revision, sandbox.revision + 1);
    assert_eq!(
        paused.lifecycle.paused_notice,
        Some(inactivity_pause_notice(900))
    );
    let events = store.sandbox_events(&sandbox.id, 0, 10).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].seq, 2);
    assert_eq!(
        events[1].kind,
        SandboxEventKind::StatusChanged {
            from: SandboxStatus::Active,
            to: SandboxStatus::Pausing,
        }
    );
    for expected in [first, second] {
        assert_eq!(
            store.get_agent(&expected.id).await.unwrap(),
            Some(expected.clone())
        );
        let document = store
            .agents
            .find_one(doc! { "_id": expected.id.as_str() })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(document.get_i64("sandbox_maintenance_fence").unwrap(), 1);
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn maintenance_rotation_captures_live_identity_and_replays_only_exact_target() {
    let (client, name, store) = database().await;
    let (sandbox, first) = fixture(&store, "sbx_rotation").await;
    let second = ready_agent(&sandbox, "agt_a");
    insert_agent(&store, &second, IDLE_AT).await;
    let mut terminal = ready_agent(&sandbox, "agt_terminal");
    terminal.status = AgentStatus::Stopped;
    insert_agent(&store, &terminal, IDLE_AT).await;
    assert_eq!(
        store
            .schedule_image_rotation(rotation(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    let stored = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
    let Some(SandboxLifecycleOperation::ImageRotation { identity, .. }) =
        &stored.lifecycle.operation
    else {
        panic!("rotation operation missing")
    };
    assert_eq!(
        identity
            .agents
            .iter()
            .map(|agent| &agent.agent_id)
            .collect::<Vec<_>>(),
        [&second.id, &first.id]
    );
    assert_eq!(
        store
            .schedule_image_rotation(rotation(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::AlreadyScheduled
    );
    let mut other = rotation(&sandbox.id);
    other.expected_runtime_version = "image-v3".into();
    assert_eq!(
        store.schedule_image_rotation(other).await.unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(stored));
    assert_eq!(
        store
            .sandbox_events(&sandbox.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    let terminal_row = store
        .agents
        .find_one(doc! { "_id": terminal.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        terminal_row.get_i64("sandbox_maintenance_fence").unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn maintenance_requests_concurrently_schedule_once() {
    let (client, name, store) = database().await;
    let (sandbox, _) = fixture(&store, "sbx_concurrent").await;
    let store = Arc::new(store);
    let results = futures_util::future::join_all((0..8).map(|_| {
        let store = store.clone();
        let request = pause(&sandbox.id);
        async move { store.schedule_inactivity_pause(request).await.unwrap() }
    }))
    .await;
    assert_eq!(
        results
            .iter()
            .filter(|result| **result == SandboxMaintenanceOutcome::Scheduled)
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| **result == SandboxMaintenanceOutcome::AlreadyScheduled)
            .count(),
        7
    );
    assert_eq!(
        store
            .sandbox_events(&sandbox.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn maintenance_rejects_missing_gapped_and_misaligned_live_logs_without_mutation() {
    let (client, name, store) = database().await;
    for scenario in ["missing", "gap", "stale_head", "identity"] {
        let (sandbox, agent) = fixture(&store, &format!("sbx_{scenario}")).await;
        match scenario {
            "missing" => {
                store
                    .agent_log
                    .delete_one(doc! { "agent_id": agent.id.as_str() })
                    .await
                    .unwrap();
            }
            "gap" => {
                let entry = AgentLogEntry {
                    agent_id: agent.id.clone(),
                    seq: 3,
                    appended_at: IDLE_AT.into(),
                    kind: AgentLogKind::Deleted,
                };
                store
                    .agent_log
                    .insert_one(log_document(&entry).unwrap())
                    .await
                    .unwrap();
            }
            "stale_head" => {
                store.agents.update_one(doc! { "_id": agent.id.as_str() },
                    doc! { "$set": { "last_seq": 2_i64, "record.conversation.last_seq": 2_i64 } })
                    .await.unwrap();
            }
            "identity" => {
                store
                    .agent_log
                    .update_one(
                        doc! { "agent_id": agent.id.as_str() },
                        doc! { "$set": { "record.agent_id": "agt_someone_else" } },
                    )
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let result = store.schedule_inactivity_pause(pause(&sandbox.id)).await;
        assert!(matches!(
            result,
            Err(SandboxStoreError::SequenceConflict | SandboxStoreError::Corrupt(_))
        ));
        assert_eq!(
            store.get_sandbox(&sandbox.id).await.unwrap(),
            Some(sandbox.clone())
        );
        assert!(store
            .sandbox_events(&sandbox.id, 0, 10)
            .await
            .unwrap()
            .is_empty());
        let row = store
            .agents
            .find_one(doc! { "_id": agent.id.as_str() })
            .await
            .unwrap()
            .unwrap();
        assert!(!row.contains_key("sandbox_maintenance_fence"));
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn maintenance_rejects_corrupt_event_heads_and_rolls_back_all_rows() {
    let (client, name, store) = database().await;
    let (sandbox, agent) = fixture(&store, "sbx_event_gap").await;
    let event = SandboxEvent {
        sandbox_id: sandbox.id.clone(),
        seq: 2,
        appended_at: IDLE_AT.into(),
        kind: SandboxEventKind::Created,
    };
    store
        .sandbox_events
        .insert_one(event_document(&event).unwrap())
        .await
        .unwrap();
    store
        .sandboxes
        .update_one(
            doc! { "_id": sandbox.id.as_str() },
            doc! { "$set": { "last_seq": 2_i64 } },
        )
        .await
        .unwrap();
    assert_eq!(
        store.schedule_inactivity_pause(pause(&sandbox.id)).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(
        store.get_sandbox(&sandbox.id).await.unwrap(),
        Some(sandbox.clone())
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    assert_eq!(
        store.sandbox_events(&sandbox.id, 0, 10).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(
        store
            .sandbox_events
            .count_documents(doc! { "sandbox_id": sandbox.id.as_str() })
            .await
            .unwrap(),
        1
    );
    let document = store
        .sandbox_events
        .find_one(doc! { "sandbox_id": sandbox.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::store::sandbox_events::read_event(&document).unwrap(),
        event
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn maintenance_agent_fence_conflicts_with_a_turn_after_the_evidence_snapshot() {
    let (client, name, store) = database().await;
    let (sandbox, ready) = fixture(&store, "sbx_turn_race").await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let evidence = evidence::live_activities(&store, std::slice::from_ref(&ready), &mut session)
        .await
        .unwrap();
    assert!(layer2_core::sandbox::is_inactivity_pause_candidate(
        &sandbox, &evidence, LATER, 900
    ));
    let mut working = ready.clone();
    working
        .transition(AgentStatus::Working, LATER.into(), None)
        .unwrap();
    working.conversation.active_turn = Some(2);
    working.conversation.last_seq = 3;
    store
        .commit_agent_conversation(AgentConversationMutation {
            command_claim: None,
            execution_fence: None,
            trigger_execution_fence: None,
            admission_guard: Some(layer2_core::sandbox::store::SandboxMutationGuard::capture(
                &sandbox,
            )),
            expected_agent_revision: ready.revision,
            expected_last_seq: ready.conversation.last_seq,
            runtime_generation: 1,
            agent: working.clone(),
            appended_at: LATER.into(),
            agent_logs: vec![
                AgentLogKind::TurnStarted {
                    model: None,
                    turn_sequence: 2,
                    origin_key: None,
                },
                AgentLogKind::OwnerMessage {
                    content: "Continue the investigation.".into(),
                },
            ],
        })
        .await
        .unwrap();
    let result = transaction::apply(
        &store,
        &MaintenanceRequest::Inactivity(pause(&sandbox.id)),
        &mut session,
    )
    .await;
    assert!(matches!(result, Err(ApplyError::Driver(error)) if is_transient(&error)));
    let _ = session.abort_transaction().await;
    assert_eq!(
        store
            .schedule_inactivity_pause(pause(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(sandbox));
    assert_eq!(store.get_agent(&ready.id).await.unwrap(), Some(working));
    assert_eq!(store.agent_log(&ready.id, 0, 10).await.unwrap().len(), 3);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn maintenance_fence_conflicts_with_a_new_log_at_the_same_agent_revision() {
    let (client, name, store) = database().await;
    let (sandbox, ready) = fixture(&store, "sbx_log_race").await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    evidence::live_activities(&store, std::slice::from_ref(&ready), &mut session)
        .await
        .unwrap();
    store
        .append_agent_log(
            &ready.id,
            LATER.into(),
            AgentLogKind::OwnerMessage {
                content: "Check the new request.".into(),
            },
        )
        .await
        .unwrap();
    let current = store.get_agent(&ready.id).await.unwrap().unwrap();
    assert_eq!(current.revision, ready.revision);
    assert_eq!(
        current.conversation.last_seq,
        ready.conversation.last_seq + 1
    );
    let result = transaction::apply(
        &store,
        &MaintenanceRequest::Inactivity(pause(&sandbox.id)),
        &mut session,
    )
    .await;
    assert!(matches!(result, Err(ApplyError::Driver(error)) if is_transient(&error)));
    let _ = session.abort_transaction().await;
    assert_eq!(
        store
            .schedule_inactivity_pause(pause(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(sandbox));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn maintenance_sandbox_write_conflicts_with_a_newly_admitted_agent() {
    let (client, name, store) = database().await;
    let (sandbox, ready) = fixture(&store, "sbx_admission_race").await;
    let workspace = Workspace {
        id: ready.workspace_id.clone(),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId("wkp_maintenance".into()),
            revision: 1,
            digest: "d".repeat(64),
            content: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory {
                    name: "maintenance".into(),
                },
                name: "maintenance".into(),
                setup_commands: vec![],
                agent_profiles: vec![],
            },
        },
        name: "maintenance".into(),
        path: "/home/cma/maintenance".into(),
        source: WorkspaceSource::Empty,
        resolved_commit: Some("a".repeat(40)),
        status: WorkspaceStatus::Ready,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, IDLE_AT.into()),
        next_worktree_number: 2,
        created_at: IDLE_AT.into(),
        ready_at: Some(IDLE_AT.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    };
    store.create_workspace(workspace.clone()).await.unwrap();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    evidence::live_activities(&store, std::slice::from_ref(&ready), &mut session)
        .await
        .unwrap();
    store
        .admit_agent(AgentAdmission {
            binding: None,
            created_by: None,
            owner: sandbox.owner.clone(),
            workspace_id: workspace.id,
            agent_id: AgentId("agt_newly_admitted".into()),
            worktree_id: WorktreeId("wkt_newly_admitted".into()),
            profile: ready.profile.clone(),
            capabilities: ready.capabilities.clone(),
            title: None,
            first_message: None,
            origin: AgentOrigin::Manual,
            creation_key: "new-during-maintenance".into(),
            event_at: LATER.into(),
        })
        .await
        .unwrap();
    let result = transaction::apply(
        &store,
        &MaintenanceRequest::Inactivity(pause(&sandbox.id)),
        &mut session,
    )
    .await;
    assert!(matches!(result, Err(ApplyError::Driver(error)) if is_transient(&error)));
    let _ = session.abort_transaction().await;
    assert_eq!(
        store
            .schedule_inactivity_pause(pause(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(
        store
            .get_sandbox(&sandbox.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SandboxStatus::Active
    );
    let row = store
        .agents
        .find_one(doc! { "_id": ready.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert!(!row.contains_key("sandbox_maintenance_fence"));
    assert_eq!(
        store
            .sandbox_events(&sandbox.id, 0, 10)
            .await
            .unwrap()
            .len(),
        2
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn maintenance_retries_transient_writes_and_reconciles_ambiguous_commits_once() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
        .expect("CHRONO_TEST_MONGODB_FAILPOINT_URI must name a failpoint-enabled replica set");
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app_name = format!("cma-maintenance-{suffix}");
    let name = format!("cma_maintenance_failpoint_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    for scenario in ["transient", "ambiguous_pause", "ambiguous_rotation"] {
        let (sandbox, agent) = fixture(&store, &format!("sbx_{scenario}")).await;
        let (times, data) = if scenario == "transient" {
            (
                1_i32,
                doc! {
                    "failCommands": ["update"], "appName": &app_name,
                    "errorCode": 112_i32, "errorLabels": ["TransientTransactionError"],
                },
            )
        } else {
            (
                5_i32,
                doc! {
                    "failCommands": ["commitTransaction"], "appName": &app_name,
                    "writeConcernError": {
                        "code": 50_i32, "codeName": "MaxTimeMSExpired",
                        "errmsg": "injected ambiguous maintenance commit",
                    },
                },
            )
        };
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": { "times": times }, "data": data,
            })
            .await
            .unwrap();
        let result = if scenario == "ambiguous_rotation" {
            store.schedule_image_rotation(rotation(&sandbox.id)).await
        } else {
            store.schedule_inactivity_pause(pause(&sandbox.id)).await
        };
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": "off",
            })
            .await
            .expect("disable failpoint before assertions");
        assert_eq!(result.unwrap(), SandboxMaintenanceOutcome::Scheduled);
        assert_eq!(
            store
                .sandbox_events(&sandbox.id, 0, 10)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .get_sandbox(&sandbox.id)
                .await
                .unwrap()
                .unwrap()
                .revision,
            2
        );
        let row = store
            .agents
            .find_one(doc! { "_id": agent.id.as_str() })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.get_i64("sandbox_maintenance_fence").unwrap(), 1);
    }
    client.database(&name).drop().await.unwrap();
}
