use layer2_core::sandbox::store::{
    AgentLogStore, AgentStore, SandboxInactivityPauseRequest, SandboxLifecycleCommand,
    SandboxLifecycleCommandStore, SandboxMaintenanceStore, SandboxMutationGuard, SandboxStore,
};
use layer2_core::sandbox::{AgentStatus, Sandbox};
use mongodb::options::ClientOptions;
use mongodb::Client;

use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::{database, fixture};
use layer2_core::sandbox::AgentLogKind;

const NOW: &str = "2026-09-05T00:30:00Z";

fn turn(parent: &Sandbox, current: &Agent) -> AgentConversationMutation {
    let mut next = current.clone();
    next.transition(AgentStatus::Working, NOW.into(), None)
        .unwrap();
    let turn_sequence = current.conversation.last_seq + 1;
    next.conversation.active_turn = Some(turn_sequence);
    next.conversation.last_seq += 2;
    AgentConversationMutation {
        settings_fence: None,
        command_claim: None,
        execution_fence: None,
        trigger_execution_fence: None,
        admission_guard: Some(SandboxMutationGuard::capture(parent)),
        expected_agent_revision: current.revision,
        expected_last_seq: current.conversation.last_seq,
        runtime_generation: current.conversation.runtime_generation.unwrap(),
        agent: next,
        appended_at: NOW.into(),
        agent_logs: vec![
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "Run the next investigation.".into(),
            },
        ],
    }
}

fn runtime_update(current: &Agent) -> AgentConversationMutation {
    let mut next = current.clone();
    next.revision += 1;
    next.conversation.last_seq += 1;
    next.updated_at = NOW.into();
    AgentConversationMutation {
        settings_fence: None,
        command_claim: None,
        execution_fence: None,
        trigger_execution_fence: None,
        admission_guard: None,
        expected_agent_revision: current.revision,
        expected_last_seq: current.conversation.last_seq,
        runtime_generation: current.conversation.runtime_generation.unwrap(),
        agent: next,
        appended_at: NOW.into(),
        agent_logs: vec![AgentLogKind::AgentMessage {
            content: "Recorded runtime output.".into(),
        }],
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn conversation_admission_rejects_a_parent_read_from_before_maintenance() {
    let (client, name, store) = database().await;
    let (parent, agent) = fixture(&store, "sbx_late_admission").await;
    let request = turn(&parent, &agent);
    store
        .schedule_inactivity_pause(SandboxInactivityPauseRequest {
            sandbox_id: parent.id.clone(),
            requested_at: NOW.into(),
            pause_after_seconds: 900,
        })
        .await
        .unwrap();
    let paused = store.get_sandbox(&parent.id).await.unwrap().unwrap();
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(agent.clone())
    );
    assert_eq!(
        store.commit_agent_conversation(request).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(agent.clone())
    );
    assert_eq!(store.agent_log(&agent.id, 0, 100).await.unwrap().len(), 1);
    let output = runtime_update(&agent);
    assert_eq!(
        store
            .commit_agent_conversation(output.clone())
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(store.get_sandbox(&parent.id).await.unwrap(), Some(paused));
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(output.agent)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn conversation_admission_requires_parent_guard_for_input_and_preserves_public_revision() {
    let (client, name, store) = database().await;
    let (parent, agent) = fixture(&store, "sbx_admission_guard").await;
    let mut request = turn(&parent, &agent);
    request.admission_guard = None;
    assert_eq!(
        store.commit_agent_conversation(request).await,
        Err(SandboxStoreError::Conflict)
    );
    let request = turn(&parent, &agent);
    store
        .commit_agent_conversation(request.clone())
        .await
        .unwrap();
    assert_eq!(
        store.get_sandbox(&parent.id).await.unwrap(),
        Some(parent.clone())
    );
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(request.agent)
    );
    let document = store
        .sandboxes
        .find_one(doc! { "_id": parent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        document
            .get_i64("agent_conversation_admission_fence")
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn conversation_admission_parent_write_conflicts_with_lifecycle_after_snapshot_read() {
    let (client, name, store) = database().await;
    let (parent, agent) = fixture(&store, "sbx_parent_write_race").await;
    let request = turn(&parent, &agent);
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    store
        .sandboxes
        .find_one(doc! { "_id": parent.id.as_str() })
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            parent.id.clone(),
            parent.owner.clone(),
            NOW.into(),
            None,
        ))
        .await
        .unwrap();
    let result = apply_mutation(&store, &request, &mut session).await;
    assert!(matches!(result, Err(ApplyError::Driver(error)) if is_transient(&error)));
    let _ = session.abort_transaction().await;
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(agent.clone())
    );
    assert_eq!(store.agent_log(&agent.id, 0, 100).await.unwrap().len(), 1);
    assert_eq!(
        store.commit_agent_conversation(request).await,
        Err(SandboxStoreError::Conflict)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn conversation_admission_reconciles_ambiguous_input_commit_without_repeating_parent_fence() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app_name = format!("cma-input-fence-{suffix}");
    let name = format!("cma_input_fence_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (parent, agent) = fixture(&store, "sbx_input_ambiguous").await;
    let request = turn(&parent, &agent);
    admin.database("admin").run_command(doc! {
        "configureFailPoint": "failCommand", "mode": { "times": 5_i32 },
        "data": { "failCommands": ["commitTransaction"], "appName": &app_name,
            "writeConcernError": { "code": 50_i32, "codeName": "MaxTimeMSExpired", "errmsg": "injected ambiguous input commit" } },
    }).await.unwrap();
    let result = store.commit_agent_conversation(request.clone()).await;
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .expect("disable failpoint before assertions");
    assert_eq!(result.unwrap().len(), 2);
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(request.agent)
    );
    assert_eq!(store.agent_log(&agent.id, 0, 100).await.unwrap().len(), 3);
    let document = store
        .sandboxes
        .find_one(doc! { "_id": parent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        document
            .get_i64("agent_conversation_admission_fence")
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}
