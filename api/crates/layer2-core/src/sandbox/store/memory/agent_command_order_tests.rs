use super::super::agent_admission_tests::{admission, created, ready_store, NOW};
use super::InMemorySandboxStore;
use crate::ids::{AgentCommandId, AgentResponseId};
use crate::sandbox::agent_io::*;
use crate::sandbox::store::AgentAdmissionStore;
use crate::sandbox::{Agent, AgentStatus};

const DEADLINE: &str = "2026-09-05T00:05:00Z";

async fn fixture(active_turn: Option<u64>) -> (InMemorySandboxStore, Agent) {
    let store = ready_store(1).await;
    let mut agent = created(store.admit_agent(admission("fifo")).await.unwrap()).agent;
    agent.status = if active_turn.is_some() {
        AgentStatus::Working
    } else {
        AgentStatus::Ready
    };
    agent.conversation.active_turn = active_turn;
    store.lock().agents.insert(agent.id.clone(), agent.clone());
    (store, agent)
}

fn insert(
    store: &InMemorySandboxStore,
    agent: &Agent,
    command: AgentCommand,
    held: bool,
) -> AdmittedAgentCommand {
    let mut state = store.lock();
    let lane = state
        .agent_command_orders
        .entry(agent.id.clone())
        .or_default();
    lane.last_order += 1;
    let order = lane.last_order;
    let actor = if held {
        AgentActor::Channel {
            bridge_id: "bridge-fifo".into(),
            grant_id: "grant-fifo".into(),
            platform: "telegram".into(),
            chat_id: "chat-fifo".into(),
            sender_id: "sender-fifo".into(),
        }
    } else {
        AgentActor::Human {
            subject: agent.owner.clone(),
        }
    };
    let record = AdmittedAgentCommand {
        queued_input: None,
        receipt: AdmissionReceipt {
            envelope: AgentCommandEnvelope {
                schema_version: AGENT_IO_SCHEMA_VERSION,
                command_id: AgentCommandId::generate(),
                agent_id: agent.id.clone(),
                owner: AgentOwner::Person {
                    subject: agent.owner.clone(),
                },
                actor,
                source: AgentCommandSource {
                    kind: if held {
                        AgentSourceKind::Channel
                    } else {
                        AgentSourceKind::UserInterface
                    },
                    reference: format!("source-{order}"),
                },
                correlation_id: format!("correlation-{order}"),
                request_fingerprint: command.fingerprint().unwrap(),
                authorization: AgentAuthorizationRevision {
                    authority_id: if held { "channel-grant" } else { "human-login" }.into(),
                    policy_revision: 1,
                    grant_revision: None,
                },
                command,
            },
            response_id: Some(AgentResponseId::generate()),
            admitted_at: NOW.into(),
            agent_order: order,
            event_cursor: 0,
        },
        key_sha256: command_key_digest(&format!("key-{order}")).unwrap(),
        queue_deadline: DEADLINE.into(),
        status: if held {
            AgentResponseStatus::WaitingAction
        } else {
            AgentResponseStatus::Queued
        },
        revision: 1,
        target_turn: agent.conversation.active_turn,
        work: CommandWorkState::default(),
    };
    record.validate().unwrap();
    state
        .agent_commands
        .insert(record.receipt.envelope.command_id.clone(), record.clone());
    record
}

async fn claim(store: &InMemorySandboxStore, agent: &Agent) -> Option<AdmittedAgentCommand> {
    store
        .claim_next_command(CommandClaimRequest {
            agent_id: agent.id.clone(),
            holder: "fifo-worker".into(),
            now: NOW.into(),
            expires_at: "2026-09-05T00:00:30Z".into(),
        })
        .await
        .unwrap()
}

async fn refuse(store: &InMemorySandboxStore, command: &AdmittedAgentCommand) {
    store
        .reject_command(
            &command.work.lease.as_ref().unwrap().claim,
            CommandFailureCode::AuthorityRevoked,
            NOW.into(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn channel_hold_retains_its_position_between_ui_messages_until_released() {
    let (store, agent) = fixture(None).await;
    let before = insert(&store, &agent, AgentCommand::message("before"), false);
    let held = insert(&store, &agent, AgentCommand::message("channel"), true);
    let after = insert(&store, &agent, AgentCommand::message("after"), false);
    let first = claim(&store, &agent).await.unwrap();
    assert_eq!(first.receipt, before.receipt);
    refuse(&store, &first).await;
    for _ in 0..2 {
        assert!(claim(&store, &agent).await.is_none());
    }
    assert_eq!(
        store
            .get_command(&held.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(held.clone())
    );
    // Seed the result of the dedicated Channel release transaction, which is
    // exercised against actual Mongo by channel_admission::feature_gates.
    {
        let mut state = store.lock();
        let released = state
            .agent_commands
            .get_mut(&held.receipt.envelope.command_id)
            .unwrap();
        released.status = AgentResponseStatus::Queued;
        released.revision += 1;
        released.validate().unwrap();
    }
    let second = claim(&store, &agent).await.unwrap();
    assert_eq!(second.receipt, held.receipt);
    refuse(&store, &second).await;
    assert_eq!(claim(&store, &agent).await.unwrap().receipt, after.receipt);
}

#[tokio::test]
async fn held_channel_messages_still_allow_controls_for_the_captured_active_turn() {
    for control in [
        AgentCommand::Interrupt,
        AgentCommand::Steer {
            input: AgentInput::text("adjust"),
        },
    ] {
        let (store, agent) = fixture(Some(7)).await;
        let held = insert(&store, &agent, AgentCommand::message("channel"), true);
        let later = insert(&store, &agent, AgentCommand::message("later UI"), false);
        let control = insert(&store, &agent, control, false);
        let selected = claim(&store, &agent).await.unwrap();
        assert_eq!(selected.receipt, control.receipt);
        assert_eq!(selected.target_turn, Some(7));
        refuse(&store, &selected).await;
        assert!(claim(&store, &agent).await.is_none());
        assert_eq!(
            store
                .get_command(&held.receipt.envelope.command_id)
                .await
                .unwrap(),
            Some(held)
        );
        assert_eq!(
            store
                .get_command(&later.receipt.envelope.command_id)
                .await
                .unwrap(),
            Some(later)
        );
    }
}
