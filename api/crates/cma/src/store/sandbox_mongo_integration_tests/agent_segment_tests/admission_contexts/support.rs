use super::super::super::command_ledger_tests::request;
use super::super::retained_assessment::ready_source;
use super::super::worker_discovery::finish_legacy_connection;
use super::*;
use crate::store::agent_log_segments::{publication, writer_gate as gate};
use crate::store::sandbox_mongo::{commit_transaction, CommitOutcome};
use layer2_core::sandbox::store::{AgentLogPublicationIdentity, SandboxMutationGuard};
use layer2_core::sandbox::{AgentOperationDelivery, AgentOperationIntent, AgentOperationRecord};
use std::time::{Duration, UNIX_EPOCH};
fn after(at: &str, seconds: u64) -> String {
    layer2_core::rfc3339_at(
        UNIX_EPOCH + Duration::from_secs(layer2_core::seconds_from_rfc3339(at).unwrap() + seconds),
    )
}

pub(super) fn claim_request(agent: &Agent, holder: &str, now: &str) -> CommandClaimRequest {
    CommandClaimRequest {
        agent_id: agent.id.clone(),
        holder: holder.into(),
        now: now.into(),
        expires_at: after(now, 30),
    }
}

pub(super) fn live_request(agent: &Agent, parent: &Sandbox, key: &str) -> CommandReservation {
    let mut value = request(agent, parent, key);
    value.admitted_at = layer2_core::now_rfc3339();
    value.queue_deadline = after(&value.admitted_at, 300);
    value
}

pub(super) fn message_mutation(
    agent: &Agent,
    parent: &Sandbox,
    claim: &CommandClaim,
) -> AgentConversationMutation {
    let now = layer2_core::now_rfc3339();
    let mut next = agent.clone();
    let sequence = agent.conversation.last_seq + 1;
    let turn = sequence + 1;
    next.transition(AgentStatus::Working, now.clone(), None)
        .unwrap();
    next.conversation.active_turn = Some(turn);
    next.conversation.operation = Some(
        AgentOperationRecord::new(
            sequence,
            1,
            turn,
            AgentOperationIntent::send_message("Inspect this."),
            now.clone(),
        )
        .unwrap(),
    );
    next.conversation.last_seq += 4;
    AgentConversationMutation {
        settings_fence: None,
        command_claim: Some(claim.clone()),
        execution_fence: None,
        trigger_execution_fence: None,
        admission_guard: Some(SandboxMutationGuard::capture(parent)),
        expected_agent_revision: agent.revision,
        expected_last_seq: agent.conversation.last_seq,
        runtime_generation: 1,
        agent: next,
        appended_at: now,
        agent_logs: vec![
            AgentLogKind::OperationDeliveryChanged {
                operation_sequence: sequence,
                operation: layer2_core::sandbox::AgentOperationKind::SendMessage,
                state: AgentOperationDelivery::DispatchFenced,
            },
            AgentLogKind::TurnStarted {
                model: Some(agent.settings().model),
                turn_sequence: turn,
                origin_key: Some(format!("command:{}", claim.command_id.as_str())),
            },
            AgentLogKind::OwnerMessage {
                content: "Inspect this.".into(),
            },
            AgentLogKind::StatusChanged {
                from: AgentStatus::Ready,
                to: AgentStatus::Working,
            },
        ],
    }
}

pub(super) fn record(result: CommandReservationResult) -> AdmittedAgentCommand {
    match result {
        CommandReservationResult::Created(record) | CommandReservationResult::Existing(record) => {
            *record
        }
    }
}
pub(super) async fn active(
    suffix: &str,
) -> (
    Client,
    String,
    MongoSandboxStore,
    Agent,
    AdmittedAgentCommand,
) {
    let (client, name) = database(suffix).await;
    segments::schema::migrate(&client.database(&name))
        .await
        .unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let ready = finish_legacy_connection(&store, ready_source(&store, suffix).await).await;
    let parent = store.get_sandbox(&ready.sandbox_id).await.unwrap().unwrap();
    let source = record(
        store
            .reserve_command(live_request(&ready, &parent, "source"))
            .await
            .unwrap(),
    );
    let claimed = store
        .claim_next_command(claim_request(
            &ready,
            "source-worker",
            &layer2_core::now_rfc3339(),
        ))
        .await
        .unwrap()
        .unwrap();
    let claim = claimed.work.lease.as_ref().unwrap().claim.clone();
    store
        .commit_agent_conversation(message_mutation(&ready, &parent, &claim))
        .await
        .unwrap();
    store
        .release_command_claim(&claim, layer2_core::now_rfc3339())
        .await
        .unwrap();
    let working = store.get_agent(&ready.id).await.unwrap().unwrap();
    (client, name, store, working, source)
}
pub(super) async fn adopt(
    client: &Client,
    name: &str,
    store: &MongoSandboxStore,
    agent: Agent,
) -> Agent {
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    gate::replace(
        &client.database(name),
        gate::GateName::Segments,
        0,
        gate::Policy {
            enabled: true,
            agents: vec![agent.id.to_string()],
            ..Default::default()
        },
        &mut session,
    )
    .await
    .unwrap();
    assert!(matches!(
        commit_transaction(&mut session).await,
        CommitOutcome::Committed
    ));
    let kind = AgentLogKind::AvailableCommandsSet { commands: vec![] };
    let identity = AgentLogPublicationIdentity {
        key: "context-adoption".into(),
        appended_at: NOW.into(),
        fingerprint: publication::request_digest(&kind)
            .unwrap()
            .bytes
            .try_into()
            .unwrap(),
    };
    store
        .append_agent_log_keyed(&agent.id, &identity, kind)
        .await
        .unwrap();
    let row = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_i64("document_schema_version").unwrap(), 2);
    store.get_agent(&agent.id).await.unwrap().unwrap()
}
