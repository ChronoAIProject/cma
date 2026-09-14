use std::sync::Arc;

use super::fixtures::*;
use crate::ids::{AgentCommandId, AgentResponseId};
use crate::sandbox::agent_io::*;
use crate::sandbox::store::{AgentStore, SandboxMutationGuard};
use crate::sandbox::*;

fn reservation(agent: &Agent, parent: &Sandbox, key: &str) -> CommandReservation {
    let command = AgentCommand::message("Inspect this.");
    CommandReservation {
        envelope: AgentCommandEnvelope {
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
                authority_id: "personal-owner".into(),
                policy_revision: 1,
                grant_revision: None,
            },
            command,
        },
        key_sha256: command_key_digest(key).unwrap(),
        response_id: Some(AgentResponseId::generate()),
        admitted_at: NOW.into(),
        queue_deadline: LATEST.into(),
        queue_capacity: 2,
        expected_agent_revision: agent.revision,
        parent: SandboxMutationGuard::capture(parent),
    }
}

async fn fixture() -> (Arc<InMemorySandboxStore>, Agent, Sandbox) {
    let store = Arc::new(InMemorySandboxStore::default());
    let parent = seed_active_parent(&store).await;
    let agent = test_agent(AgentStatus::Working, None);
    store.create_agent(agent.clone()).await.unwrap();
    (store, agent, parent)
}

fn record(result: CommandReservationResult) -> AdmittedAgentCommand {
    match result {
        CommandReservationResult::Created(value) | CommandReservationResult::Existing(value) => {
            *value
        }
    }
}

#[tokio::test]
async fn racing_retries_reserve_one_input_response_and_order() {
    let (store, agent, parent) = fixture().await;
    let a = reservation(&agent, &parent, "request-one");
    let mut b = reservation(&agent, &parent, "request-one");
    b.envelope.authorization.policy_revision = 2;
    b.envelope.source.kind = AgentSourceKind::Api;
    let (one, two) = tokio::join!(store.reserve_command(a), store.reserve_command(b.clone()));
    let (one, two) = (one.unwrap(), two.unwrap());
    assert_ne!(
        matches!(one, CommandReservationResult::Created(_)),
        matches!(two, CommandReservationResult::Created(_))
    );
    let original = record(one);
    assert_eq!(original, record(two));
    assert_eq!(original.receipt.agent_order, 1);
    assert_eq!(
        store
            .get_command(&original.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(original.clone())
    );
    b.envelope.command = AgentCommand::message("Different input");
    b.envelope.request_fingerprint = b.envelope.command.fingerprint().unwrap();
    assert_eq!(
        store.reserve_command(b).await.unwrap_err(),
        CommandLedgerError::IdempotencyConflict
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
}

#[tokio::test]
async fn independent_authorities_share_one_bounded_order_and_retries_keep_their_slot() {
    let (store, agent, parent) = fixture().await;
    let first = reservation(&agent, &parent, "request-one");
    let first_record = record(store.reserve_command(first.clone()).await.unwrap());
    let mut second = reservation(&agent, &parent, "request-one");
    second.envelope.authorization.authority_id = "another-verified-grant".into();
    second.envelope.source.kind = AgentSourceKind::Channel;
    second.envelope.actor = AgentActor::Channel {
        bridge_id: "bridge-one".into(),
        grant_id: "grant-one".into(),
        platform: "fixture".into(),
        chat_id: "chat-one".into(),
        sender_id: "sender-one".into(),
    };
    let second_record = record(store.reserve_command(second).await.unwrap());
    assert_eq!(second_record.receipt.agent_order, 2);
    assert_eq!(
        store
            .reserve_command(reservation(&agent, &parent, "request-three"))
            .await
            .unwrap_err(),
        CommandLedgerError::QueueFull
    );
    assert_eq!(
        record(store.reserve_command(first).await.unwrap()),
        first_record
    );
}

#[tokio::test]
async fn stale_or_foreign_admission_has_no_receipt_or_consumed_order() {
    let (store, agent, parent) = fixture().await;
    let mut stale = reservation(&agent, &parent, "request-one");
    stale.expected_agent_revision += 1;
    let id = stale.envelope.command_id.clone();
    assert_eq!(
        store.reserve_command(stale).await.unwrap_err(),
        CommandLedgerError::Conflict
    );
    assert!(store.get_command(&id).await.unwrap().is_none());
    let mut foreign = reservation(&agent, &parent, "request-one");
    foreign.envelope.owner = AgentOwner::Person {
        subject: crate::SubjectId::new("someone-else"),
    };
    assert_eq!(
        store.reserve_command(foreign).await.unwrap_err(),
        CommandLedgerError::NotFound
    );
    let valid = record(
        store
            .reserve_command(reservation(&agent, &parent, "request-one"))
            .await
            .unwrap(),
    );
    assert_eq!(valid.receipt.agent_order, 1);
}
