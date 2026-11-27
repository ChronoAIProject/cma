use futures_util::TryStreamExt;
mod admission_tests;
mod audit_tests;
mod budget_tests;
mod ceilings_tests;
mod claim_observation_tests;
mod contextual_suggestions;
mod discovery_tests;
pub(in crate::store) mod execution_tests;
mod observability_tests;
mod operator_stop_tests;
mod platform_admission_evidence_tests;
mod platform_commitment_tests;
mod queued_cancellation_tests;
mod queued_input_tests;
mod refusal_compatibility_tests;
mod reservation_outcomes;
mod rest_receipt_recovery_tests;
mod sandbox_controls_tests;
mod startup_failure_tests;
mod suggested_reply_tests;
mod targeted_control_tests;
mod task_audit_tests;
pub(in crate::store) mod work_tests;
use layer2_core::ids::{AgentCommandId, AgentResponseId};
use layer2_core::sandbox::agent_io::*;
use layer2_core::sandbox::store::SandboxMutationGuard;

use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::{
    active_sandbox, insert_agent, ready_agent,
};

pub(super) fn request(agent: &Agent, parent: &Sandbox, key: &str) -> CommandReservation {
    let command = AgentCommand::message("Inspect this.");
    CommandReservation {
        binding: None,
        authority_deadline: None,
        admission_audit: None,
        envelope: AgentCommandEnvelope {
            continuation: None,
            schema_version: AGENT_IO_SCHEMA_VERSION,
            command_id: AgentCommandId::generate(),
            agent_id: agent.id.clone(),
            owner: AgentOwner::Person {
                subject: agent.owner.clone(),
            },
            actor: AgentActor::Human {
                subject: agent.owner.clone(),
            },
            source: AgentCommandSource {
                kind: AgentSourceKind::UserInterface,
                reference: "interaction-one".into(),
            },
            correlation_id: "correlation-one".into(),
            request_fingerprint: command.fingerprint().unwrap(),
            authorization: AgentAuthorizationRevision {
                replay_principal: None,
                authority_id: "personal-owner".into(),
                policy_revision: 1,
                grant_revision: None,
            },
            command,
        },
        key_sha256: command_key_digest(key).unwrap(),
        response_id: Some(AgentResponseId::generate()),
        admitted_at: NOW.into(),
        queue_deadline: TWO_MINUTES.into(),
        queue_capacity: 2,
        expected_agent_revision: agent.revision,
        parent: SandboxMutationGuard::capture(parent),
        policy: layer2_core::resource_access::ResourcePolicyFence {
            sandbox_id: parent.id.clone(),
            sandbox_created_at: parent.created_at.clone(),
            policy_revision: 0,
        },
    }
}

async fn fixture(name: &str, client: &Client) -> (MongoSandboxStore, Agent, Sandbox) {
    let store = MongoSandboxStore::open(client, name).await.unwrap();
    let parent = active_sandbox("sbx_commands");
    let agent = ready_agent(&parent, "agt_commands");
    store.create_sandbox(parent.clone()).await.unwrap();
    insert_agent(&store, &agent, NOW).await;
    (store, agent, parent)
}

pub(in crate::store) fn record(result: CommandReservationResult) -> AdmittedAgentCommand {
    match result {
        CommandReservationResult::Created(value) | CommandReservationResult::Existing(value) => {
            *value
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn scope_key_lookup_recovers_after_restart_without_admitting_or_mutating_work() {
    let (client, name) = database("command_lookup").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let reservation = request(&agent, &parent, "legacy-operation-key");
    let scope = reservation.scope();
    let key = reservation.key_sha256.clone();
    assert_eq!(
        store.get_admitted_command(&scope, &key).await.unwrap(),
        None
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        0
    );
    let original = record(store.reserve_command(reservation).await.unwrap());
    let command = store
        .agent_commands
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    let order = store
        .agent_command_orders
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    drop(store);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened.get_admitted_command(&scope, &key).await.unwrap(),
        Some(original)
    );
    assert_eq!(
        reopened
            .get_admitted_command(&scope, &command_key_digest("different-key").unwrap())
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        reopened
            .get_admitted_command(&command_key_digest("different-scope").unwrap(), &key)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        reopened.agent_commands.find_one(doc! {}).await.unwrap(),
        Some(command)
    );
    assert_eq!(
        reopened
            .agent_command_orders
            .find_one(doc! {})
            .await
            .unwrap(),
        Some(order)
    );
    assert_eq!(
        reopened
            .agent_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(reopened.get_agent(&agent.id).await.unwrap(), Some(agent));
    assert_eq!(
        reopened.get_sandbox(&parent.id).await.unwrap(),
        Some(parent)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn concurrent_replicas_reserve_one_receipt_and_retry_after_reopening_returns_it() {
    let (client, name) = database("command_retry").await;
    let (one, agent, parent) = fixture(&name, &client).await;
    let two = MongoSandboxStore::open(&client, &name).await.unwrap();
    let a = request(&agent, &parent, "request-one");
    let mut b = request(&agent, &parent, "request-one");
    b.envelope.authorization.policy_revision = 2;
    b.envelope.source.kind = AgentSourceKind::Api;
    let (first, retry) = tokio::join!(
        one.reserve_command(a.clone()),
        two.reserve_command(b.clone())
    );
    let (first, retry) = (first.unwrap(), retry.unwrap());
    assert_ne!(
        matches!(first, CommandReservationResult::Created(_)),
        matches!(retry, CommandReservationResult::Created(_))
    );
    let original = record(first);
    assert_eq!(original, record(retry));
    assert_eq!(original.receipt.agent_order, 1);
    assert_eq!(
        one.agent_commands.count_documents(doc! {}).await.unwrap(),
        1
    );
    assert_eq!(one.get_agent(&agent.id).await.unwrap(), Some(agent.clone()));
    assert_eq!(one.get_sandbox(&parent.id).await.unwrap(), Some(parent));
    drop(one);
    drop(two);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(record(reopened.reserve_command(a).await.unwrap()), original);
    b.envelope.command = AgentCommand::message("Changed input");
    b.envelope.request_fingerprint = b.envelope.command.fingerprint().unwrap();
    assert_eq!(
        reopened.reserve_command(b).await.unwrap_err(),
        CommandLedgerError::IdempotencyConflict
    );
    assert_eq!(
        reopened
            .get_command(&original.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(original)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn all_authorities_share_one_atomic_queue_bound_and_retries_do_not_consume_capacity() {
    let (client, name) = database("command_queue").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let first = request(&agent, &parent, "request-one");
    let original = record(store.reserve_command(first.clone()).await.unwrap());
    let mut second = request(&agent, &parent, "request-one");
    second.envelope.authorization.authority_id = "another-verified-grant".into();
    let second_record = record(store.reserve_command(second).await.unwrap());
    assert_eq!(second_record.receipt.agent_order, 2);
    assert_eq!(
        store
            .reserve_command(request(&agent, &parent, "third"))
            .await
            .unwrap_err(),
        CommandLedgerError::QueueFull
    );
    assert_eq!(
        record(store.reserve_command(first).await.unwrap()),
        original
    );
    assert_eq!(
        store
            .agent_command_orders
            .find_one(doc! { "_id": agent.id.as_str() })
            .await
            .unwrap()
            .unwrap()
            .get_i64("last_order")
            .unwrap(),
        2
    );
    let indexes: Vec<_> = store
        .agent_commands
        .list_indexes()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert!(indexes.iter().all(|index| index
        .options
        .as_ref()
        .is_none_or(|options| options.expire_after.is_none())));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn stale_admission_rolls_back_every_fence_and_corrupt_indexed_receipts_fail_closed() {
    let (client, name) = database("command_integrity").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let before = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    let mut stale = request(&agent, &parent, "request-one");
    stale.expected_agent_revision += 1;
    assert_eq!(
        store.reserve_command(stale).await.unwrap_err(),
        CommandLedgerError::KnownAborted
    );
    assert_eq!(
        store
            .agents
            .find_one(doc! { "_id": agent.id.as_str() })
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        0
    );
    assert_eq!(
        store
            .agent_command_orders
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let valid = record(
        store
            .reserve_command(request(&agent, &parent, "request-one"))
            .await
            .unwrap(),
    );
    assert_eq!(valid.receipt.agent_order, 1);
    let id = &valid.receipt.envelope.command_id;
    store
        .agent_commands
        .update_one(
            doc! { "_id": id.as_str() },
            doc! { "$set": { "agent_order": 3_i64 } },
        )
        .await
        .unwrap();
    assert_eq!(
        store.get_command(id).await.unwrap_err(),
        CommandLedgerError::Corrupt
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn principal_scoped_command_replay_survives_authority_change_and_reopen() {
    let (client, name) = database("command_principal_replay").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let mut first = request(&agent, &parent, "principal-retry");
    first.envelope.authorization.authority_id = "authority-a".into();
    first.envelope.authorization.replay_principal = Some("principal-p".into());
    let CommandReservationResult::Created(original) =
        store.reserve_command(first.clone()).await.unwrap()
    else {
        panic!("expected a new command");
    };
    drop(store);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let stored = reopened
        .get_command(&original.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&stored, original.as_ref());
    assert_eq!(
        stored
            .receipt
            .envelope
            .authorization
            .replay_principal
            .as_deref(),
        Some("principal-p")
    );
    let mut retry = first;
    retry.envelope.command_id = AgentCommandId::generate();
    retry.response_id = Some(AgentResponseId::generate());
    retry.envelope.authorization.authority_id = "authority-b".into();
    let CommandReservationResult::Existing(replayed) =
        reopened.reserve_command(retry).await.unwrap()
    else {
        panic!("expected the original command");
    };
    assert_eq!(replayed, original);
    assert_eq!(
        replayed.receipt.envelope.command_id,
        original.receipt.envelope.command_id
    );
    assert_eq!(replayed.receipt.response_id, original.receipt.response_id);
    assert_eq!(
        replayed.receipt.envelope.authorization.authority_id,
        "authority-a"
    );
    assert_eq!(
        reopened
            .agent_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}
