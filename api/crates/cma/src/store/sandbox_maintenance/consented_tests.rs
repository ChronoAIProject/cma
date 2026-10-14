//! Real replica-set transactions prove consent/quietness and lifecycle receipt atomicity.
use layer2_core::sandbox::agent_access::AgentAccessError;
use layer2_core::sandbox::store::{
    AgentConversationMutation, AgentConversationStore, AgentStore, SandboxEventStore,
    SandboxMutationGuard, SandboxStore,
};
use layer2_core::sandbox::{AgentLogKind, AgentStatus, SandboxStatus};
use mongodb::bson::doc;

use super::*;
use crate::store::agent_access_requests::tests::approved_fixture;

async fn record(store: &MongoSandboxStore, id: &str) -> AgentAccessRecord {
    access::read(
        &store
            .agent_access_requests
            .find_one(doc! {"_id":id})
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn consented_pause_atomically_records_operation_and_replays_after_store_restart() {
    let (client, name, store, approved) = approved_fixture().await;
    let lease = approved.lease.as_ref().unwrap();
    let sandbox_id = &approved.requester.sandbox_id;
    let before = store.get_sandbox(sandbox_id).await.unwrap().unwrap();
    let events = store
        .sandbox_events(sandbox_id, 0, 100)
        .await
        .unwrap()
        .len();
    assert_eq!(
        store.schedule_access_request_pause(lease).await.unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    let paused = store.get_sandbox(sandbox_id).await.unwrap().unwrap();
    assert_eq!(paused.status, SandboxStatus::Pausing);
    assert_eq!(paused.revision, before.revision + 1);
    let saved = record(&store, &approved.id).await;
    assert_eq!(saved.phase, AgentAccessPhase::Pausing);
    assert_eq!(
        saved.progress.pause_operation_id.as_deref(),
        Some(
            paused
                .lifecycle
                .operation
                .as_ref()
                .unwrap()
                .authority()
                .operation_id
                .as_str()
        )
    );
    assert_eq!(saved.decision, approved.decision);
    assert_eq!(saved.requester, approved.requester);
    let restarted = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        restarted
            .schedule_access_request_pause(lease)
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::AlreadyScheduled
    );
    assert_eq!(
        restarted.get_sandbox(sandbox_id).await.unwrap().unwrap(),
        paused
    );
    assert_eq!(
        restarted
            .sandbox_events(sandbox_id, 0, 100)
            .await
            .unwrap()
            .len(),
        events + 1
    );
    assert_eq!(record(&restarted, &approved.id).await, saved);
    let mut completed = paused;
    completed.status = SandboxStatus::Paused;
    completed.runtime = None;
    completed.lifecycle.operation = None;
    completed.work = layer2_core::sandbox::SandboxWorkState::new(
        layer2_core::sandbox::SandboxWorkStep::None,
        layer2_core::now_rfc3339(),
    );
    completed.revision += 1;
    super::revival_tests::save_parent(&store, &completed).await;
    assert_eq!(
        store.schedule_access_request_pause(lease).await.unwrap(),
        SandboxMaintenanceOutcome::AlreadyScheduled
    );
    completed.lifecycle.next_authority += 2;
    completed.revision += 1;
    super::revival_tests::save_parent(&store, &completed).await;
    assert_eq!(
        store.schedule_access_request_pause(lease).await,
        Err(AgentAccessError::Conflict)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn consented_pause_refuses_expired_replaced_or_denied_lease_without_lifecycle_change() {
    let (client, name, store, approved) = approved_fixture().await;
    let lease = approved.lease.as_ref().unwrap();
    let before = store
        .get_sandbox(&approved.requester.sandbox_id)
        .await
        .unwrap();
    for kind in ["expired", "replacement", "denied"] {
        let mut changed = approved.clone();
        match kind {
            "expired" => changed.lease.as_mut().unwrap().expires_at = 1,
            "replacement" => changed.lease.as_mut().unwrap().token = "newer-worker".into(),
            _ => {
                changed.phase = AgentAccessPhase::Denied;
                changed.decision = Some(AgentAccessDecision::Denied {
                    command_key_hash: "d".repeat(64),
                });
                changed.approved_at = None;
                changed.lease = None;
            }
        }
        changed.revision += 1;
        store
            .agent_access_requests
            .replace_one(
                doc! {"_id":&approved.id},
                access::document(&changed).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            store.schedule_access_request_pause(lease).await,
            Err(AgentAccessError::Conflict)
        );
        assert_eq!(
            store
                .get_sandbox(&approved.requester.sandbox_id)
                .await
                .unwrap(),
            before
        );
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn consented_pause_agent_fence_loses_to_a_new_turn_without_partial_pause() {
    let (client, name, store, approved) = approved_fixture().await;
    let lease = approved.lease.as_ref().unwrap();
    let sandbox = store
        .get_sandbox(&approved.requester.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let ready = store
        .get_agent(&approved.requester.agent_id)
        .await
        .unwrap()
        .unwrap();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    store
        .agent_access_requests
        .find_one(doc! {"_id":&approved.id})
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    let mut working = ready.clone();
    let at = layer2_core::now_rfc3339();
    working
        .transition(AgentStatus::Working, at.clone(), None)
        .unwrap();
    let turn = ready.conversation.last_seq + 1;
    working.conversation.active_turn = Some(turn);
    working.conversation.last_seq = turn + 1;
    store
        .commit_agent_conversation(AgentConversationMutation {
            command_claim: None,
            execution_fence: None,
            trigger_execution_fence: None,
            admission_guard: Some(SandboxMutationGuard::capture(&sandbox)),
            expected_agent_revision: ready.revision,
            expected_last_seq: ready.conversation.last_seq,
            runtime_generation: ready.conversation.runtime_generation.unwrap(),
            agent: working.clone(),
            appended_at: at,
            agent_logs: vec![
                AgentLogKind::TurnStarted {
                    turn_sequence: turn,
                    origin_key: None,
                },
                AgentLogKind::OwnerMessage {
                    content: "Continue the active task.".into(),
                },
            ],
        })
        .await
        .unwrap();
    let attempted = apply(&store, lease, Action::Pause, &mut session).await;
    assert!(matches!(attempted, Err(error) if error.transient()));
    let _ = session.abort_transaction().await;
    assert_eq!(
        store.schedule_access_request_pause(lease).await.unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(sandbox));
    assert_eq!(store.get_agent(&ready.id).await.unwrap(), Some(working));
    assert_eq!(
        record(&store, &approved.id).await.phase,
        AgentAccessPhase::WaitingQuiet
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn consented_pause_loses_to_a_replaced_lease_in_the_same_snapshot() {
    let (client, name, store, approved) = approved_fixture().await;
    let lease = approved.lease.as_ref().unwrap();
    let before = store
        .get_sandbox(&approved.requester.sandbox_id)
        .await
        .unwrap();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    store
        .agent_access_requests
        .find_one(doc! {"_id":&approved.id})
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    let mut winner = approved.clone();
    winner.revision += 1;
    winner.lease.as_mut().unwrap().token = "newer-lease".into();
    store
        .agent_access_requests
        .replace_one(
            doc! {"_id":&approved.id},
            access::document(&winner).unwrap(),
        )
        .await
        .unwrap();
    let attempted = apply(&store, lease, Action::Pause, &mut session).await;
    assert!(matches!(attempted, Err(error) if error.transient()));
    let _ = session.abort_transaction().await;
    assert_eq!(record(&store, &approved.id).await, winner);
    assert_eq!(
        store
            .get_sandbox(&approved.requester.sandbox_id)
            .await
            .unwrap(),
        before
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn consented_pause_refuses_replaced_native_conversation_before_effects() {
    let (client, name, store, approved) = approved_fixture().await;
    let lease = approved.lease.as_ref().unwrap();
    let before = store
        .get_sandbox(&approved.requester.sandbox_id)
        .await
        .unwrap();
    store
        .agents
        .update_one(
            doc! {"_id":approved.requester.agent_id.as_str()},
            doc! {"$set":{"record.conversation.conversation_id":"replaced-native-conversation"}},
        )
        .await
        .unwrap();
    assert!(store.schedule_access_request_pause(lease).await.is_err());
    assert_eq!(
        store
            .get_sandbox(&approved.requester.sandbox_id)
            .await
            .unwrap(),
        before
    );
    assert_eq!(
        record(&store, &approved.id).await.phase,
        AgentAccessPhase::WaitingQuiet
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn consented_pause_reads_every_live_agent_and_rejects_incomplete_logs() {
    let (client, name, store, approved) = approved_fixture().await;
    let lease = approved.lease.as_ref().unwrap();
    let before = store
        .get_sandbox(&approved.requester.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let mut other = super::super::mongo_integration_tests::ready_agent(&before, "agt_other_busy");
    other
        .transition(AgentStatus::Working, layer2_core::now_rfc3339(), None)
        .unwrap();
    other.conversation.active_turn = Some(1);
    super::super::mongo_integration_tests::insert_agent(
        &store,
        &other,
        &layer2_core::now_rfc3339(),
    )
    .await;
    assert_eq!(
        store.schedule_access_request_pause(lease).await.unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    store
        .agents
        .delete_one(doc! {"_id":other.id.as_str()})
        .await
        .unwrap();
    store
        .agent_log
        .delete_many(doc! {"agent_id":approved.requester.agent_id.as_str()})
        .await
        .unwrap();
    assert!(store.schedule_access_request_pause(lease).await.is_err());
    assert_eq!(store.get_sandbox(&before.id).await.unwrap(), Some(before));
    assert_eq!(
        record(&store, &approved.id).await.phase,
        AgentAccessPhase::WaitingQuiet
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn consented_pause_retries_transient_writes_and_reconciles_ambiguous_atomic_commit() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
        .expect("failpoint-enabled replica set required");
    for ambiguous in [false, true] {
        let (client, name, original, approved) = approved_fixture().await;
        let app_name = format!("cma-consented-{}", uuid::Uuid::new_v4().simple());
        let mut options = mongodb::options::ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app_name.clone());
        let named_client = mongodb::Client::with_options(options).unwrap();
        let store = MongoSandboxStore::open(&named_client, &name).await.unwrap();
        let before = original
            .get_sandbox(&approved.requester.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let events = original
            .sandbox_events(&before.id, 0, 100)
            .await
            .unwrap()
            .len();
        let (times, data) = if ambiguous {
            (
                5_i32,
                doc! {"failCommands":["commitTransaction"], "appName":&app_name,
                "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected ambiguous consented pause"}},
            )
        } else {
            (
                1_i32,
                doc! {"failCommands":["update"],"appName":&app_name,"errorCode":112_i32,"errorLabels":["TransientTransactionError"]},
            )
        };
        named_client
            .database("admin")
            .run_command(
                doc! {"configureFailPoint":"failCommand","mode":{"times":times},"data":data},
            )
            .await
            .unwrap();
        let result = store
            .schedule_access_request_pause(approved.lease.as_ref().unwrap())
            .await;
        named_client
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
            .await
            .unwrap();
        assert_eq!(result.unwrap(), SandboxMaintenanceOutcome::Scheduled);
        assert_eq!(
            record(&store, &approved.id).await.phase,
            AgentAccessPhase::Pausing
        );
        assert_eq!(
            store
                .get_sandbox(&before.id)
                .await
                .unwrap()
                .unwrap()
                .revision,
            before.revision + 1
        );
        assert_eq!(
            store
                .sandbox_events(&before.id, 0, 100)
                .await
                .unwrap()
                .len(),
            events + 1
        );
        client.database(&name).drop().await.unwrap();
    }
}
