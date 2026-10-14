use layer2_core::ids::SubjectId;
use layer2_core::sandbox::store::{AgentLogStore, AgentStore, SandboxEventStore, SandboxStore};
use layer2_core::sandbox::{
    Agent, AgentLogKind, AgentOperationDelivery, AgentOperationIntent, AgentOperationRecord,
    AgentStatus, AgentStopReason, AgentWorkStep, RuntimeBinding, SandboxCheckpointConversation,
    SandboxCheckpointRef, SandboxEventKind, SandboxImageRotationPhase, SandboxLifecycleOperation,
    SandboxPausePhase, SandboxReviveCause, SandboxRotationIdentity, SandboxStatus, SandboxWorkStep,
    TypedFailure,
};
use layer3_adapter::{CheckpointReceipt, ConversationSnapshot, WriterState};
use mongodb::options::ClientOptions;
use mongodb::Client;
use sha2::{Digest, Sha256};

use super::*;
use crate::store::agents::agent_replacement_fields;
use crate::store::sandbox_maintenance::mongo_integration_tests::{
    active_sandbox, database, fixture, insert_agent, ready_agent,
};
use crate::store::sandbox_mongo::start_transaction;
use crate::store::sandboxes::{sandbox_document, sandbox_replacement_fields};

const BEFORE: &str = "2026-09-05T00:00:00Z";
const OBSERVED: &str = "2026-09-05T00:30:00Z";

mod channel_home_retention;

fn loss(sandbox: &Sandbox) -> SandboxRuntimeLossRequest {
    SandboxRuntimeLossRequest {
        sandbox_id: sandbox.id.clone(),
        expected_revision: sandbox.revision,
        expected_runtime: sandbox.runtime.clone(),
        observed_at: OBSERVED.into(),
    }
}

fn checkpoint(agents: &[Agent]) -> SandboxCheckpointRef {
    let conversations = agents
        .iter()
        .map(|agent| {
            let conversation_id = agent.conversation.conversation_id.clone().unwrap();
            let archive = format!("conversation archive for {}", agent.id).into_bytes();
            SandboxCheckpointConversation::new(
                agent.id.clone(),
                &conversation_id,
                ConversationSnapshot {
                    conversation_id: conversation_id.clone(),
                    archive_sha256: format!("{:x}", Sha256::digest(&archive)),
                    archive,
                },
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    SandboxCheckpointRef::seal(
        &CheckpointReceipt {
            epoch: 1,
            object_reference: "checkpoint/runtime-loss/1".into(),
            sha256: "a".repeat(64),
            writers: WriterState::Held,
        },
        &conversations,
        BEFORE,
    )
    .unwrap()
}

async fn checkpoint_fixture(store: &MongoSandboxStore, id: &str) -> (Sandbox, Agent) {
    let (mut sandbox, agent) = fixture(store, id).await;
    sandbox.lifecycle.checkpoints.latest = Some(checkpoint(std::slice::from_ref(&agent)));
    let expected = sandbox.revision;
    sandbox.revision += 1;
    store.save_sandbox(expected, sandbox.clone()).await.unwrap();
    (sandbox, agent)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_authority_pages_every_owner_and_lifecycle_state_and_validates_rows() {
    let (client, name, store) = database().await;
    let states = [
        SandboxStatus::Provisioning,
        SandboxStatus::Installing,
        SandboxStatus::Active,
        SandboxStatus::Pausing,
        SandboxStatus::Paused,
        SandboxStatus::Reviving,
        SandboxStatus::CredentialRotating,
        SandboxStatus::Rotating,
        SandboxStatus::Failed,
        SandboxStatus::Retiring,
        SandboxStatus::Retired,
    ];
    for (index, status) in states.into_iter().enumerate().rev() {
        let mut value = active_sandbox(&format!("sbx_authority_{index:02}"));
        value.owner = SubjectId(format!("owner-{index}"));
        value.lifecycle.checkpoints.latest = Some(checkpoint(&[]));
        match status {
            SandboxStatus::Pausing => value.begin_pause(BEFORE.into(), None).unwrap(),
            SandboxStatus::Reviving => {
                value.status = SandboxStatus::Paused;
                value.runtime = None;
                value.work.schedule(SandboxWorkStep::None, BEFORE.into());
                value
                    .begin_revive(SandboxReviveCause::ExplicitRequest, BEFORE.into())
                    .unwrap();
            }
            SandboxStatus::Rotating => {
                let identity = SandboxRotationIdentity::capture(&value, &[]).unwrap();
                value
                    .begin_image_rotation("new-image".into(), identity, BEFORE.into())
                    .unwrap();
            }
            _ => value.status = status,
        }
        if status == SandboxStatus::Paused {
            value.runtime = None;
        }
        if status == SandboxStatus::Failed {
            value.failure = Some(TypedFailure {
                class: "runtime_lost".into(),
                detail: "gone".into(),
            });
        }
        if status == SandboxStatus::Retired {
            value.retired_at = Some(BEFORE.into());
        }
        store
            .sandboxes
            .insert_one(sandbox_document(&value, 0).unwrap())
            .await
            .unwrap();
    }
    let mut cursor = None;
    let mut found = Vec::new();
    loop {
        let page = store
            .sandbox_runtime_authority(&PageRequest::new(3, cursor).unwrap())
            .await
            .unwrap();
        found.extend(page.items);
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        found
            .iter()
            .map(|sandbox| sandbox.status)
            .collect::<Vec<_>>(),
        states
    );
    assert!(found.last().unwrap().runtime.is_some());
    for limit in [0, MAX_PAGE_SIZE + 1] {
        assert_eq!(
            store
                .sandbox_runtime_authority(&PageRequest {
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
            doc! { "_id": found[0].id.as_str() },
            doc! { "$set": { "record.id": "sbx_different" } },
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .sandbox_runtime_authority(&PageRequest::default())
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_loss_recovers_checkpoint_agents_preserves_history_and_stops_newer_agents() {
    let (client, name, store) = database().await;
    let (sandbox, mut live) = checkpoint_fixture(&store, "sbx_recover").await;
    for kind in [
        AgentLogKind::OperationDeliveryChanged {
            operation_sequence: 2,
            operation: layer2_core::sandbox::AgentOperationKind::SendMessage,
            state: AgentOperationDelivery::DispatchFenced,
        },
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 3,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "Preserve this owner's work.".into(),
        },
    ] {
        store
            .append_agent_log(&live.id, BEFORE.into(), kind)
            .await
            .unwrap();
    }
    live = store.get_agent(&live.id).await.unwrap().unwrap();
    live.status = AgentStatus::Working;
    live.conversation.active_turn = Some(3);
    live.conversation.operation = Some(
        AgentOperationRecord::new(
            2,
            1,
            3,
            AgentOperationIntent::send_message("Preserve this owner's work."),
            BEFORE.into(),
        )
        .unwrap(),
    );
    live.work
        .schedule(AgentWorkStep::PublishConnection, BEFORE.into());
    live.work
        .claim("old-worker".into(), BEFORE, OBSERVED.into())
        .unwrap();
    store
        .agents
        .update_one(
            doc! { "_id": live.id.as_str() },
            doc! { "$set": agent_replacement_fields(&live).unwrap() },
        )
        .await
        .unwrap();
    let newer = ready_agent(&sandbox, "agt_newer_checkpoint");
    insert_agent(&store, &newer, BEFORE).await;
    let mut terminal = ready_agent(&sandbox, "agt_already_stopped");
    terminal
        .transition(
            AgentStatus::Stopped,
            BEFORE.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    insert_agent(&store, &terminal, BEFORE).await;
    let old_logs = store.agent_log(&live.id, 0, 100).await.unwrap();
    assert_eq!(
        store.recover_runtime_loss(loss(&sandbox)).await.unwrap(),
        SandboxRuntimeLossOutcome::RecoveryScheduled
    );
    let recovered = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
    assert_eq!(recovered.status, SandboxStatus::Reviving);
    assert_eq!(recovered.runtime, None);
    assert_eq!(
        recovered.lifecycle.checkpoints,
        sandbox.lifecycle.checkpoints
    );
    assert_eq!(recovered.profile, sandbox.profile);
    assert_eq!(recovered.credential, sandbox.credential);
    let parked = store.get_agent(&live.id).await.unwrap().unwrap();
    assert_eq!(parked.status, AgentStatus::Paused);
    assert_eq!(parked.conversation.runtime_generation, Some(2));
    assert_eq!(parked.conversation.active_turn, None);
    assert_eq!(parked.work.lease, None);
    assert_eq!(parked.work.step, AgentWorkStep::None);
    let operation = parked.conversation.operation.unwrap();
    assert_eq!(operation.state, AgentOperationDelivery::RuntimeLost);
    assert_eq!(operation.write_confirmed_at, None);
    let logs = store.agent_log(&live.id, 0, 100).await.unwrap();
    assert_eq!(&logs[..old_logs.len()], old_logs.as_slice());
    assert_eq!(logs.last().unwrap().seq, parked.conversation.last_seq);
    assert!(logs.iter().any(|entry| matches!(
        entry.kind,
        AgentLogKind::RuntimeGenerationRetired {
            previous_generation: 1,
            runtime_generation: 2,
        }
    )));
    let stopped = store.get_agent(&newer.id).await.unwrap().unwrap();
    assert_eq!(stopped.status, AgentStatus::Stopped);
    assert_eq!(stopped.stop_reason, Some(AgentStopReason::RuntimeLost));
    assert_eq!(
        store.get_agent(&terminal.id).await.unwrap(),
        Some(terminal.clone())
    );
    let terminal_row = store
        .agents
        .find_one(doc! { "_id": terminal.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(terminal_row.get_i64("sandbox_recovery_fence").unwrap(), 1);
    let events = store.sandbox_events(&sandbox.id, 0, 100).await.unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events.last().unwrap().seq, 3);
    assert_eq!(
        store.recover_runtime_loss(loss(&sandbox)).await.unwrap(),
        SandboxRuntimeLossOutcome::Superseded
    );
    assert_eq!(store.agent_log(&live.id, 0, 100).await.unwrap(), logs);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_loss_without_checkpoint_fails_bound_and_unbound_active_sandboxes() {
    let (client, name, store) = database().await;
    for bound in [true, false] {
        let (mut sandbox, agent) = fixture(&store, &format!("sbx_no_checkpoint_{bound}")).await;
        if !bound {
            sandbox.runtime = None;
            store
                .sandboxes
                .update_one(
                    doc! { "_id": sandbox.id.as_str() },
                    doc! { "$set": sandbox_replacement_fields(&sandbox).unwrap() },
                )
                .await
                .unwrap();
        }
        assert_eq!(
            store.recover_runtime_loss(loss(&sandbox)).await.unwrap(),
            SandboxRuntimeLossOutcome::Failed
        );
        let failed = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
        assert_eq!(failed.status, SandboxStatus::Failed);
        assert_eq!(failed.failure.unwrap().class, "runtime_lost");
        assert_eq!(failed.runtime, None);
        assert_eq!(failed.work.step, SandboxWorkStep::None);
        assert_eq!(
            store.get_agent(&agent.id).await.unwrap().unwrap().status,
            AgentStatus::Stopped
        );
        let events = store.sandbox_events(&sandbox.id, 0, 10).await.unwrap();
        assert!(matches!(
            events.last().unwrap().kind,
            SandboxEventKind::Failed { .. }
        ));
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_loss_resumes_mid_pause_revive_and_rotation_without_replaying_old_authority() {
    let (client, name, store) = database().await;
    for mode in ["pause", "revive", "rotation"] {
        let (mut sandbox, agent) = checkpoint_fixture(&store, &format!("sbx_mid_{mode}")).await;
        let revision = sandbox.revision;
        match mode {
            "pause" => sandbox.begin_pause(BEFORE.into(), None).unwrap(),
            "rotation" => {
                let identity =
                    SandboxRotationIdentity::capture(&sandbox, std::slice::from_ref(&agent))
                        .unwrap();
                sandbox
                    .begin_image_rotation("image-v2".into(), identity, BEFORE.into())
                    .unwrap();
            }
            "revive" => {
                sandbox.status = SandboxStatus::Paused;
                sandbox.runtime = None;
                sandbox.work.schedule(SandboxWorkStep::None, BEFORE.into());
                sandbox
                    .begin_revive(SandboxReviveCause::ExplicitRequest, BEFORE.into())
                    .unwrap();
                sandbox.runtime = Some(RuntimeBinding {
                    boot_id: None,
                    runtime_id: "lost-replacement".into(),
                    runtime_version: "image-v1".into(),
                    provision_generation: Some(2),
                    bound_at: BEFORE.into(),
                });
                sandbox.lifecycle.runtime_generation_high_water = 2;
            }
            _ => unreachable!(),
        }
        store.save_sandbox(revision, sandbox.clone()).await.unwrap();
        let old_authority = sandbox.lifecycle.next_authority;
        assert_eq!(
            store.recover_runtime_loss(loss(&sandbox)).await.unwrap(),
            SandboxRuntimeLossOutcome::RecoveryScheduled
        );
        let next = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
        assert_eq!(next.runtime, None);
        assert!(next.lifecycle.next_authority > old_authority);
        match next.lifecycle.operation.unwrap() {
            SandboxLifecycleOperation::Pause {
                phase,
                checkpoint_epoch,
                ..
            } => {
                assert_eq!(phase, SandboxPausePhase::PublishPaused);
                assert_eq!(checkpoint_epoch, Some(1));
            }
            SandboxLifecycleOperation::ImageRotation {
                phase,
                checkpoint_epoch,
                target_runtime_version,
                ..
            } => {
                assert_eq!(phase, SandboxImageRotationPhase::CheckpointVerification);
                assert_eq!(checkpoint_epoch, Some(1));
                assert_eq!(target_runtime_version, "image-v2");
            }
            SandboxLifecycleOperation::Revive { cause, .. } => {
                assert_eq!(cause, SandboxReviveCause::RuntimeLost)
            }
        }
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_loss_rejects_stale_runtime_identity_and_every_incomplete_log_or_event_stream() {
    let (client, name, store) = database().await;
    let (sandbox, agent) = checkpoint_fixture(&store, "sbx_stale").await;
    for field in ["revision", "runtime_id", "runtime_version", "bound_at"] {
        let mut request = loss(&sandbox);
        match field {
            "revision" => request.expected_revision += 1,
            "runtime_id" => request
                .expected_runtime
                .as_mut()
                .unwrap()
                .runtime_id
                .push('x'),
            "runtime_version" => request
                .expected_runtime
                .as_mut()
                .unwrap()
                .runtime_version
                .push('x'),
            "bound_at" => request.expected_runtime.as_mut().unwrap().bound_at = OBSERVED.into(),
            _ => unreachable!(),
        }
        assert_eq!(
            store.recover_runtime_loss(request).await.unwrap(),
            SandboxRuntimeLossOutcome::Superseded
        );
    }
    assert_eq!(
        store.get_sandbox(&sandbox.id).await.unwrap(),
        Some(sandbox.clone())
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    for terminal in [true, false] {
        let (sandbox, mut agent) =
            checkpoint_fixture(&store, &format!("sbx_corrupt_{terminal}")).await;
        if terminal {
            agent
                .transition(
                    AgentStatus::Stopped,
                    BEFORE.into(),
                    Some(AgentStopReason::Owner),
                )
                .unwrap();
            store
                .agents
                .update_one(
                    doc! { "_id": agent.id.as_str() },
                    doc! { "$set": agent_replacement_fields(&agent).unwrap() },
                )
                .await
                .unwrap();
        }
        store
            .agent_log
            .delete_one(doc! { "agent_id": agent.id.as_str() })
            .await
            .unwrap();
        assert_eq!(
            store.recover_runtime_loss(loss(&sandbox)).await,
            Err(SandboxStoreError::SequenceConflict)
        );
        assert_eq!(
            store.get_sandbox(&sandbox.id).await.unwrap(),
            Some(sandbox.clone())
        );
        assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
        assert!(store
            .sandbox_events(&sandbox.id, 0, 10)
            .await
            .unwrap()
            .is_empty());
    }
    let (sandbox, agent) = checkpoint_fixture(&store, "sbx_corrupt_events").await;
    store
        .sandboxes
        .update_one(
            doc! { "_id": sandbox.id.as_str() },
            doc! { "$set": { "last_seq": 1_i64 } },
        )
        .await
        .unwrap();
    assert_eq!(
        store.recover_runtime_loss(loss(&sandbox)).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(sandbox));
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_loss_retries_a_concurrent_log_head_and_fences_terminal_agent_evidence() {
    let (client, name, store) = database().await;
    for terminal in [true, false] {
        let (sandbox, mut agent) =
            checkpoint_fixture(&store, &format!("sbx_race_{terminal}")).await;
        if terminal {
            agent
                .transition(
                    AgentStatus::Stopped,
                    BEFORE.into(),
                    Some(AgentStopReason::Owner),
                )
                .unwrap();
            store
                .agents
                .update_one(
                    doc! { "_id": agent.id.as_str() },
                    doc! { "$set": agent_replacement_fields(&agent).unwrap() },
                )
                .await
                .unwrap();
        }
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        agents::load_agents(&store, &sandbox.id, &mut session)
            .await
            .unwrap();
        store
            .append_agent_log(
                &agent.id,
                OBSERVED.into(),
                AgentLogKind::OwnerMessage {
                    content: "Accepted immediately before recovery.".into(),
                },
            )
            .await
            .unwrap();
        let result = transaction::apply(&store, &loss(&sandbox), &mut session).await;
        assert!(matches!(result, Err(ApplyError::Driver(error)) if is_transient(&error)));
        let _ = session.abort_transaction().await;
        assert_eq!(
            store.recover_runtime_loss(loss(&sandbox)).await.unwrap(),
            SandboxRuntimeLossOutcome::RecoveryScheduled
        );
        let logs = store.agent_log(&agent.id, 0, 100).await.unwrap();
        assert!(
            matches!(&logs[1].kind, AgentLogKind::OwnerMessage { content } if content == "Accepted immediately before recovery.")
        );
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn runtime_loss_rolls_back_failed_log_writes_and_reconciles_ambiguous_commits_once() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app_name = format!("cma-runtime-loss-{suffix}");
    let name = format!("cma_runtime_loss_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (sandbox, agent) = checkpoint_fixture(&store, "sbx_rollback").await;
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": { "times": 1_i32 },
            "data": { "failCommands": ["insert"], "appName": &app_name, "errorCode": 2_i32 },
        })
        .await
        .unwrap();
    let failed = store.recover_runtime_loss(loss(&sandbox)).await;
    disable_failpoint(&admin).await;
    assert!(matches!(failed, Err(SandboxStoreError::Unavailable(_))));
    assert_eq!(
        store.get_sandbox(&sandbox.id).await.unwrap(),
        Some(sandbox.clone())
    );
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(agent.clone())
    );
    assert_eq!(store.agent_log(&agent.id, 0, 100).await.unwrap().len(), 1);
    assert!(store
        .sandbox_events(&sandbox.id, 0, 100)
        .await
        .unwrap()
        .is_empty());
    for mode in ["transient", "failed", "revive"] {
        let (mut sandbox, agent) = if mode == "failed" {
            fixture(&store, "sbx_unknown_failed").await
        } else {
            checkpoint_fixture(&store, &format!("sbx_unknown_{mode}")).await
        };
        if mode == "revive" {
            sandbox.status = SandboxStatus::Paused;
            sandbox.runtime = None;
            sandbox.work.schedule(SandboxWorkStep::None, BEFORE.into());
            let revision = sandbox.revision;
            sandbox
                .begin_revive(SandboxReviveCause::ExplicitRequest, BEFORE.into())
                .unwrap();
            sandbox.runtime = Some(RuntimeBinding {
                boot_id: None,
                runtime_id: "replacement".into(),
                runtime_version: "image-v2".into(),
                provision_generation: Some(2),
                bound_at: BEFORE.into(),
            });
            sandbox.lifecycle.runtime_generation_high_water = 2;
            store.save_sandbox(revision, sandbox.clone()).await.unwrap();
            let mut parked = agent.clone();
            parked.status = AgentStatus::Paused;
            store
                .agents
                .update_one(
                    doc! { "_id": parked.id.as_str() },
                    doc! { "$set": agent_replacement_fields(&parked).unwrap() },
                )
                .await
                .unwrap();
        }
        let (times, data) = if mode == "transient" {
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
                    "writeConcernError": { "code": 50_i32, "codeName": "MaxTimeMSExpired", "errmsg": "injected ambiguous runtime-loss commit" },
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
        let result = store.recover_runtime_loss(loss(&sandbox)).await;
        disable_failpoint(&admin).await;
        assert_eq!(
            result.unwrap(),
            if mode == "failed" {
                SandboxRuntimeLossOutcome::Failed
            } else {
                SandboxRuntimeLossOutcome::RecoveryScheduled
            }
        );
        let recovered = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
        assert_eq!(recovered.revision, sandbox.revision + 1);
        assert_eq!(recovered.runtime, None);
        let logs = store.agent_log(&agent.id, 0, 100).await.unwrap();
        assert_eq!(
            store.recover_runtime_loss(loss(&sandbox)).await.unwrap(),
            SandboxRuntimeLossOutcome::Superseded
        );
        assert_eq!(store.agent_log(&agent.id, 0, 100).await.unwrap(), logs);
        if mode == "revive" {
            assert!(store
                .sandbox_events(&sandbox.id, 0, 100)
                .await
                .unwrap()
                .is_empty());
        }
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_loss_concurrent_identical_observations_publish_one_recovery() {
    let (client, name, store) = database().await;
    let (sandbox, agent) = checkpoint_fixture(&store, "sbx_concurrent_loss").await;
    let results = futures_util::future::join_all((0..8).map(|_| {
        let store = store.clone();
        let request = loss(&sandbox);
        async move { store.recover_runtime_loss(request).await.unwrap() }
    }))
    .await;
    assert_eq!(
        results
            .iter()
            .filter(|outcome| **outcome == SandboxRuntimeLossOutcome::RecoveryScheduled)
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|outcome| **outcome == SandboxRuntimeLossOutcome::Superseded)
            .count(),
        7
    );
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap().unwrap().revision,
        agent.revision + 1
    );
    assert_eq!(
        store
            .get_sandbox(&sandbox.id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        sandbox.revision + 1
    );
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

async fn disable_failpoint(client: &Client) {
    client
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .expect("disable failpoint before assertions");
}
