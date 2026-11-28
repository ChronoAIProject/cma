use super::*;
use crate::store::agent_access_fixtures as fixtures;
use layer2_core::sandbox::store::AgentStore;
use layer2_core::{RuntimeCapabilities, RuntimeCapability};

#[path = "notification_drift_tests.rs"]
mod drift;

fn agent() -> Agent {
    let sandbox = fixtures::sandbox("notify-select");
    let workspace = fixtures::workspace(&sandbox, "notify-select");
    let mut agent = fixtures::admission(&workspace, "notify-select")
        .materialize(&workspace)
        .unwrap()
        .agent;
    agent.status = AgentStatus::Ready;
    agent.capabilities =
        RuntimeCapabilities::new([RuntimeCapability::Conversation, RuntimeCapability::Steer]);
    agent
}

#[test]
fn notification_choice_pins_busy_turn_and_reselects_only_a_new_observation() {
    let mut agent = agent();
    let quiet = choose(&agent, None, false).unwrap();
    assert_eq!(quiet.target_turn, None);
    agent.status = AgentStatus::Working;
    agent.conversation.active_turn = Some(7);
    assert_eq!(
        choose(&agent, Some(&quiet), false),
        Err(AgentAccessError::Conflict)
    );
    agent.revision += 1;
    let busy = choose(&agent, Some(&quiet), false).unwrap();
    assert_eq!(busy.target_turn, Some(7));
    assert_eq!(busy.attempt, 1);
    agent.revision += 1;
    let refreshed = choose(&agent, Some(&busy), false).unwrap();
    assert_eq!(refreshed.attempt, busy.attempt);
    assert_eq!(refreshed.target_turn, busy.target_turn);
    agent.status = AgentStatus::Ready;
    agent.conversation.active_turn = None;
    agent.revision += 1;
    let after_turn = choose(&agent, Some(&refreshed), false).unwrap();
    assert_eq!(after_turn.target_turn, None);
    assert_eq!(after_turn.attempt, 2);
    agent.status = AgentStatus::Working;
    agent.conversation.active_turn = Some(8);
    agent.capabilities = RuntimeCapabilities::new([RuntimeCapability::Conversation]);
    assert_eq!(choose(&agent, None, false), Err(AgentAccessError::Conflict));
}

async fn notifying_fixture() -> (
    mongodb::Client,
    String,
    std::sync::Arc<MongoSandboxStore>,
    AgentAccessRecord,
) {
    let (client, name, store, mut record) = super::super::tests::approved_fixture().await;
    let (_, sandbox) = store.verify_access_request_lineage(&record).await.unwrap();
    record.phase = AgentAccessPhase::Notifying;
    record.progress.hot_update = Some(AgentAccessHotUpdate {
        runtime: sandbox.runtime.clone().unwrap(),
        lifecycle_next_authority: sandbox.lifecycle.next_authority,
    });
    record.progress.grant_generation_floor = Some(0);
    record.progress.delivered_credential = sandbox.credential.current.clone();
    record.progress.delivered_runtime = sandbox.runtime;
    store
        .agent_access_requests
        .replace_one(doc! {"_id":&record.id}, document(&record).unwrap())
        .await
        .unwrap();
    (client, name, store, record)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_notification_choice_survives_restart_and_refuses_stale_worker() {
    let (client, name, store, record) = notifying_fixture().await;
    let lease = record.lease.as_ref().unwrap();
    let selected = store.prepare_access_notification(lease).await.unwrap();
    let intent = selected.progress.notification_intent.as_ref().unwrap();
    assert_eq!(intent.target_turn, None);
    assert!(selected.progress.notification_command_id.is_none());
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened.prepare_access_notification(lease).await.unwrap(),
        selected
    );
    let mut agent = store
        .get_agent(&record.requester.agent_id)
        .await
        .unwrap()
        .unwrap();
    agent.revision += 1;
    store
        .save_agent(agent.revision - 1, agent.clone())
        .await
        .unwrap();
    let refreshed = reopened.prepare_access_notification(lease).await.unwrap();
    let next = refreshed.progress.notification_intent.as_ref().unwrap();
    assert_eq!(next.agent_revision, agent.revision);
    assert_eq!(next.attempt, intent.attempt);
    assert_eq!(refreshed.notification_key(), selected.notification_key());
    store
        .release_access_request(lease, std::time::Duration::ZERO)
        .await
        .unwrap();
    let replacement = reopened
        .claim_access_request("replacement-notifier")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        store.prepare_access_notification(lease).await,
        Err(AgentAccessError::Conflict)
    );
    assert_eq!(
        reopened
            .prepare_access_notification(replacement.lease.as_ref().unwrap())
            .await
            .unwrap()
            .progress,
        refreshed.progress
    );
    client.database(&name).drop().await.unwrap();
}

fn command(record: &AgentAccessRecord) -> AdmittedAgentCommand {
    use layer2_core::ids::{AgentCommandId, AgentResponseId};
    use layer2_core::sandbox::agent_io::*;
    let intent = record.progress.notification_intent.as_ref().unwrap();
    let command = record.notification_command().unwrap();
    let Some(AgentAccessDecision::Approved { authorization, .. }) = &record.decision else {
        panic!()
    };
    AdmittedAgentCommand {
        queued_input: None,
        receipt: AdmissionReceipt {
            envelope: AgentCommandEnvelope {
                continuation: None,
                schema_version: AGENT_IO_SCHEMA_VERSION,
                command_id: AgentCommandId::generate(),
                agent_id: record.requester.agent_id.clone(),
                owner: AgentOwner::Person {
                    subject: record.requester.owner.clone(),
                },
                actor: AgentActor::Human {
                    subject: record.requester.owner.clone(),
                },
                source: AgentCommandSource {
                    kind: AgentSourceKind::Api,
                    reference: "access-notification".into(),
                },
                correlation_id: "notification-test".into(),
                request_fingerprint: command.fingerprint().unwrap(),
                authorization: authorization.clone(),
                command,
            },
            response_id: intent.target_turn.is_none().then(AgentResponseId::generate),
            admitted_at: fixtures::NOW.into(),
            agent_order: u64::from(intent.attempt) + 1,
            event_cursor: 0,
        },
        key_sha256: command_key_digest(&record.notification_key()).unwrap(),
        queue_deadline: "2026-12-30T00:00:00Z".into(),
        status: AgentResponseStatus::Queued,
        revision: 1,
        target_turn: intent.target_turn,
        work: Default::default(),
    }
}

async fn set_turn(store: &MongoSandboxStore, record: &AgentAccessRecord, turn: Option<u64>) {
    let mut agent = store
        .get_agent(&record.requester.agent_id)
        .await
        .unwrap()
        .unwrap();
    agent.revision += 1;
    agent.status = if turn.is_some() {
        AgentStatus::Working
    } else {
        AgentStatus::Ready
    };
    agent.capabilities =
        RuntimeCapabilities::new([RuntimeCapability::Conversation, RuntimeCapability::Steer]);
    agent.conversation.active_turn = turn;
    store.save_agent(agent.revision - 1, agent).await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_notification_recovers_lost_steer_admission_after_turn_ended_without_new_message() {
    let (client, name, store, record) = notifying_fixture().await;
    let lease = record.lease.as_ref().unwrap();
    set_turn(&store, &record, Some(9)).await;
    let selected = store.prepare_access_notification(lease).await.unwrap();
    let command = command(&selected);
    command.validate().unwrap();
    store
        .agent_commands
        .insert_one(crate::store::agent_commands::command_document(&command).unwrap())
        .await
        .unwrap();
    // Admission completed, but its receipt never reached the access worker.
    set_turn(&store, &record, None).await;
    let recovered = store.prepare_access_notification(lease).await.unwrap();
    assert_eq!(
        recovered.progress.notification_intent,
        selected.progress.notification_intent
    );
    assert_eq!(
        recovered.progress.notification_command_id,
        Some(command.receipt.envelope.command_id)
    );
    assert!(matches!(
        recovered.notification_command().unwrap(),
        AgentCommand::Steer { .. }
    ));
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_notification_retries_only_definitely_unwritten_ended_turn() {
    use layer2_core::sandbox::agent_io::{AgentResponseStatus, CommandFailure, CommandObservation};
    let (client, name, store, record) = notifying_fixture().await;
    let lease = record.lease.as_ref().unwrap();
    set_turn(&store, &record, Some(11)).await;
    let selected = store.prepare_access_notification(lease).await.unwrap();
    let mut command = command(&selected);
    command.status = AgentResponseStatus::Failed;
    command.work.failure = Some(CommandFailure {
        code: CommandFailureCode::TargetTurnChanged,
        at: fixtures::NOW.into(),
    });
    assert!(retryable_turn_change(&command));
    for delivery in [
        AgentOperationDelivery::DispatchFenced,
        AgentOperationDelivery::WriteConfirmed,
        AgentOperationDelivery::ExecutionObserved,
        AgentOperationDelivery::RecoveryRequired,
        AgentOperationDelivery::RuntimeLost,
    ] {
        command.work.observation = Some(CommandObservation {
            event_cursor: 2,
            delivery,
        });
        assert!(
            !retryable_turn_change(&command),
            "ambiguous/delivered notification cannot be repeated: {delivery:?}"
        );
    }
    command.work.observation = None;
    command.validate().unwrap();
    store
        .agent_commands
        .insert_one(crate::store::agent_commands::command_document(&command).unwrap())
        .await
        .unwrap();
    set_turn(&store, &record, None).await;
    let retry = store.prepare_access_notification(lease).await.unwrap();
    assert!(retry.progress.notification_command_id.is_none());
    assert!(matches!(
        retry.notification_command().unwrap(),
        AgentCommand::Message { .. }
    ));
    let intent = retry.progress.notification_intent.as_ref().unwrap();
    assert_eq!(intent.attempt, 1);
    assert_eq!(
        intent.previous_commands,
        vec![command.receipt.envelope.command_id]
    );
    assert_ne!(retry.notification_key(), selected.notification_key());
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        1
    );
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened.prepare_access_notification(lease).await.unwrap(),
        retry
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_notification_turn_reselection_fences_an_inflight_old_admission_snapshot() {
    use layer2_core::sandbox::agent_io::{
        AgentCommandStore, CommandLedgerError, CommandReservation,
    };
    use layer2_core::sandbox::store::SandboxMutationGuard;
    let (client, name, store, record) = notifying_fixture().await;
    let lease = record.lease.as_ref().unwrap();
    set_turn(&store, &record, Some(13)).await;
    let selected = store.prepare_access_notification(lease).await.unwrap();
    let (agent, sandbox) = store
        .verify_access_request_lineage(&selected)
        .await
        .unwrap();
    let admitted = command(&selected);
    let request = CommandReservation {
        binding: None,
        envelope: admitted.receipt.envelope,
        key_sha256: admitted.key_sha256,
        response_id: admitted.receipt.response_id,
        admitted_at: admitted.receipt.admitted_at,
        queue_deadline: admitted.queue_deadline,
        queue_capacity: 4,
        expected_agent_revision: agent.revision,
        parent: SandboxMutationGuard::capture(&sandbox),
        policy: layer2_core::resource_access::ResourcePolicyFence {
            sandbox_id: sandbox.id.clone(),
            sandbox_created_at: sandbox.created_at.clone(),
            policy_revision: 0,
        },
        authority_deadline: None,
        admission_audit: None,
    };
    let mut in_flight = client.start_session().await.unwrap();
    start_transaction(&mut in_flight).await.unwrap();
    store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .session(&mut in_flight)
        .await
        .unwrap();
    // The old dispatch has taken its snapshot but has not reserved a command.
    // The turn ends and the access worker persists a fresh Message intent.
    set_turn(&store, &record, None).await;
    let replacement = store.prepare_access_notification(lease).await.unwrap();
    assert_ne!(selected.notification_key(), replacement.notification_key());
    assert!(matches!(
        replacement.notification_command().unwrap(),
        AgentCommand::Message { .. }
    ));
    let result =
        crate::store::agent_commands::reserve::apply(&store, &request, &mut in_flight).await;
    assert!(
        matches!(result, Err(crate::store::agent_commands::ApplyError::Driver(error)) if is_transient(&error))
    );
    let _ = in_flight.abort_transaction().await;
    // A fresh transaction rejects the old observation and acknowledges its abort.
    assert_eq!(
        store.reserve_command(request).await,
        Err(CommandLedgerError::KnownAborted)
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_owner_api_notification_recovers_the_exact_normal_command_custody() {
    let (client, name, store, mut record) = notifying_fixture().await;
    let Some(AgentAccessDecision::Approved { authorization, .. }) = &mut record.decision else {
        panic!()
    };
    authorization.authority_id = format!("owner-api:{}:2000000000", "a".repeat(64));
    store
        .agent_access_requests
        .replace_one(doc! {"_id":&record.id}, document(&record).unwrap())
        .await
        .unwrap();
    let lease = record.lease.as_ref().unwrap();
    set_turn(&store, &record, Some(9)).await;
    let selected = store.prepare_access_notification(lease).await.unwrap();
    let mut callback = command(&selected);
    callback.receipt.envelope.authorization.authority_id =
        format!("api:{}:2000000000", "a".repeat(64));
    callback.receipt.envelope.actor = layer2_core::sandbox::agent_io::AgentActor::Api {
        subject: record.requester.owner.clone(),
        delegation_id: None,
    };
    callback.validate().unwrap();
    store
        .agent_commands
        .insert_one(crate::store::agent_commands::command_document(&callback).unwrap())
        .await
        .unwrap();
    set_turn(&store, &record, None).await;
    let recovered = store.prepare_access_notification(lease).await.unwrap();
    assert_eq!(
        recovered.progress.notification_command_id,
        Some(callback.receipt.envelope.command_id)
    );
    assert_eq!(
        recovered.progress.notification_intent,
        selected.progress.notification_intent
    );
    client.database(&name).drop().await.unwrap();
}
