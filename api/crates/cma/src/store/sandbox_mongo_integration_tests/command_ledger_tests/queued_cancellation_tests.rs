use super::work_tests::{claim_request, live_request, message_mutation};
use super::*;

#[path = "queued_cancellation_race_tests.rs"]
mod races;

pub(super) async fn queued(
    name: &str,
    client: &Client,
) -> (
    MongoSandboxStore,
    Agent,
    Sandbox,
    QueuedCancellationReservation,
    AdmittedAgentCommand,
) {
    let (store, agent, parent) = fixture(name, client).await;
    let source = record(
        store
            .reserve_command(live_request(&agent, &parent, "source-queued"))
            .await
            .unwrap(),
    );
    let mut command = live_request(&agent, &parent, "queued-cancel");
    command.envelope.command = AgentCommand::Interrupt;
    command.envelope.request_fingerprint = command.envelope.command.fingerprint().unwrap();
    command.response_id = None;
    let mut audit = super::audit_tests::personal_audit(&agent, &parent);
    audit.capability = layer2_core::resource_access::ResourceCapability::OperateAgent;
    audit.observations[0].capabilities = vec![audit.capability];
    command.admission_audit = Some(audit);
    let request = QueuedCancellationReservation {
        command,
        target: QueuedCancellationTarget {
            response_id: source.receipt.response_id.clone().unwrap(),
            source_command_id: source.receipt.envelope.command_id.clone(),
            expected_command_revision: source.revision,
        },
        observed_agent_revision: agent.revision,
        agent_created_at: agent.created_at.clone(),
    };
    (store, agent, parent, request, source)
}

fn applied(outcome: QueuedCancellationOutcome) -> AdmittedQueuedCancellation {
    match outcome {
        QueuedCancellationOutcome::Applied(value) => *value,
        other => panic!("expected cancellation, got {other:?}"),
    }
}

pub(super) async fn set_test_hold(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    held: bool,
) -> Document {
    use crate::store::work_claim_authority::LEDGER_MAINTENANCE_FIELD;

    let update = if held {
        doc! {"$set": {LEDGER_MAINTENANCE_FIELD: Bson::Null}}
    } else {
        doc! {"$unset": {LEDGER_MAINTENANCE_FIELD: ""}}
    };
    let key = doc! {"_id": sandbox.id.as_str()};
    assert_eq!(
        store
            .sandboxes
            .update_one(key.clone(), update)
            .await
            .unwrap()
            .matched_count,
        1
    );
    store.sandboxes.find_one(key).await.unwrap().unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn held_sandbox_refuses_fresh_queued_cancellation_and_replays_without_writes() {
    let (client, name) = database("queued_cancellation_hold").await;
    let (store, agent, parent, request, source) = queued(&name, &client).await;
    let replica = MongoSandboxStore::open(&client, &name).await.unwrap();
    let held = set_test_hold(&store, &parent, true).await;
    assert_eq!(
        replica.cancel_queued_response(request.clone()).await,
        Err(CommandLedgerError::Conflict),
    );
    assert_eq!(
        store
            .get_admitted_command(&source.scope(), &source.key_sha256)
            .await
            .unwrap(),
        Some(source)
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        1
    );
    assert_eq!(
        store
            .sandboxes
            .find_one(doc! {"_id": parent.id.as_str()})
            .await
            .unwrap(),
        Some(held)
    );

    set_test_hold(&store, &parent, false).await;
    let cancelled = store.cancel_queued_response(request.clone()).await.unwrap();
    let held = set_test_hold(&store, &parent, true).await;
    let agent_key = doc! {"_id": agent.id.as_str()};
    let before = store.agents.find_one(agent_key.clone()).await.unwrap();
    assert_eq!(
        replica.cancel_queued_response(request).await.unwrap(),
        cancelled
    );
    assert_eq!(store.agents.find_one(agent_key).await.unwrap(), before);
    assert_eq!(
        store
            .sandboxes
            .find_one(doc! {"_id": parent.id.as_str()})
            .await
            .unwrap(),
        Some(held)
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        2
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn queued_cancellation_commits_terminal_effect_and_receipt_across_replicas() {
    let (client, name) = database("queued_cancellation").await;
    let (store, agent, parent, request, source) = queued(&name, &client).await;
    let two = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut retry = request.clone();
    retry.command.envelope.command_id = AgentCommandId::generate();
    let (one, two_result) = tokio::join!(
        store.cancel_queued_response(request.clone()),
        two.cancel_queued_response(retry)
    );
    let original = applied(one.unwrap());
    assert_eq!(applied(two_result.unwrap()), original);
    assert_eq!(original.response.status, AgentResponseStatus::Cancelled);
    assert_eq!(original.response.receipt, source.receipt);
    assert_eq!(original.response.work, CommandWorkState::default());
    assert_eq!(original.record.status, AgentResponseStatus::Completed);
    assert_eq!(original.record.target_turn, None);
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(agent.clone())
    );
    assert_eq!(store.get_sandbox(&parent.id).await.unwrap(), Some(parent));
    let raw = store
        .agent_commands
        .find_one(doc! {"_id": original.record.receipt.envelope.command_id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let legacy: AdmittedAgentCommand =
        mongodb::bson::deserialize_from_bson(raw.get("record").unwrap().clone()).unwrap();
    assert_eq!(legacy, original.record);
    assert_eq!(
        legacy.receipt.envelope.request_fingerprint,
        AgentCommand::Interrupt.fingerprint().unwrap()
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        2
    );
    assert_eq!(
        store
            .command_authorization_audits
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        applied(two.cancel_queued_response(request.clone()).await.unwrap()),
        original
    );
    let mut stale = request.clone();
    stale.command.key_sha256 = command_key_digest("other-key").unwrap();
    assert_eq!(
        two.cancel_queued_response(stale).await.unwrap(),
        QueuedCancellationOutcome::NotAdmitted
    );
    let mut changed = request;
    changed.target.expected_command_revision += 1;
    assert_eq!(
        two.cancel_queued_response(changed).await.unwrap_err(),
        CommandLedgerError::IdempotencyConflict
    );
    assert!(store
        .claim_next_command(claim_request(&agent, "worker", &layer2_core::now_rfc3339()))
        .await
        .unwrap()
        .is_none());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn queued_cancellation_releases_only_the_exact_claim_and_old_worker_cannot_bind() {
    let (client, name) = database("queued_cancellation_claim").await;
    let (store, agent, parent, mut request, _) = queued(&name, &client).await;
    let claimed = store
        .claim_next_command(claim_request(
            &agent,
            "old-worker",
            &layer2_core::now_rfc3339(),
        ))
        .await
        .unwrap()
        .unwrap();
    let claim = claimed.work.lease.as_ref().unwrap().claim.clone();
    request.target.expected_command_revision = claimed.revision;
    let lane_before = store
        .agent_command_orders
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let result = applied(store.cancel_queued_response(request).await.unwrap());
    assert_eq!(
        store
            .release_command_claim(&claim, layer2_core::now_rfc3339())
            .await
            .unwrap_err(),
        CommandLedgerError::StaleClaim
    );
    assert!(store
        .commit_agent_conversation(message_mutation(&agent, &parent, &claim))
        .await
        .is_err());
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(agent.clone())
    );
    assert_eq!(
        store.get_command(&claim.command_id).await.unwrap(),
        Some(result.response)
    );
    let lane = store
        .agent_command_orders
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lane.get("lease"), Some(&mongodb::bson::Bson::Null));
    assert_eq!(lane.get("next_fence"), lane_before.get("next_fence"));
    assert_eq!(lane.get_i64("last_order").unwrap(), 2);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn queued_cancellation_cannot_replace_a_dispatch_binding_or_stale_authority() {
    let (client, name) = database("queued_cancellation_bound").await;
    let (store, agent, parent, mut request, _) = queued(&name, &client).await;
    let claimed = store
        .claim_next_command(claim_request(&agent, "worker", &layer2_core::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    let claim = claimed.work.lease.as_ref().unwrap().claim.clone();
    store
        .commit_agent_conversation(message_mutation(&agent, &parent, &claim))
        .await
        .unwrap();
    let bound = store.get_command(&claim.command_id).await.unwrap().unwrap();
    let agent = store.get_agent(&agent.id).await.unwrap().unwrap();
    request.target.expected_command_revision = bound.revision;
    assert_eq!(
        store
            .cancel_queued_response(request.clone())
            .await
            .unwrap_err(),
        CommandLedgerError::Conflict
    );
    request.observed_agent_revision = agent.revision;
    assert_eq!(
        store.cancel_queued_response(request).await.unwrap_err(),
        CommandLedgerError::Conflict
    );
    assert_eq!(
        store.get_command(&claim.command_id).await.unwrap(),
        Some(bound)
    );
    assert_eq!(
        store
            .agent_commands
            .count_documents(doc! {"queued_cancellation": {"$exists":true}})
            .await
            .unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}
