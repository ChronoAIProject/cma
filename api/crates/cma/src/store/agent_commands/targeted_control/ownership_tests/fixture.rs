use super::*;
use layer2_core::sandbox::{
    AgentLogKind, AgentOperationDelivery, AgentOperationIntent, AgentOperationRecord, AgentStatus,
};
use std::time::{Duration, UNIX_EPOCH};

pub(super) async fn queued(store: &MongoSandboxStore) -> (Agent, QueuedCancellationReservation) {
    let (parent, agent) = seed_agent(store, "queued-owner").await;
    let source = record(
        store
            .reserve_command(live_request(&agent, &parent, "source"))
            .await
            .unwrap(),
    );
    let mut command = live_request(&agent, &parent, "cancel-key");
    command.envelope.command = AgentCommand::Interrupt;
    command.envelope.request_fingerprint = command.envelope.command.fingerprint().unwrap();
    command.response_id = None;
    let request = QueuedCancellationReservation {
        command,
        target: QueuedCancellationTarget {
            response_id: source.receipt.response_id.unwrap(),
            source_command_id: source.receipt.envelope.command_id,
            expected_command_revision: source.revision,
        },
        observed_agent_revision: agent.revision,
        agent_created_at: agent.created_at.clone(),
    };
    (agent, request)
}

pub(super) async fn targeted(store: &MongoSandboxStore) -> (Agent, TargetedControlReservation) {
    let (parent, agent) = seed_agent(store, "targeted-owner").await;
    let source = record(
        store
            .reserve_command(live_request(&agent, &parent, "source"))
            .await
            .unwrap(),
    );
    let now = layer2_core::now_rfc3339();
    let expires_at = layer2_core::rfc3339_at(
        UNIX_EPOCH + Duration::from_secs(layer2_core::seconds_from_rfc3339(&now).unwrap() + 30),
    );
    let claimed = store
        .claim_next_command(CommandClaimRequest {
            agent_id: agent.id.clone(),
            holder: "fixture-worker".into(),
            now: now.clone(),
            expires_at,
        })
        .await
        .unwrap()
        .unwrap();
    let claim = claimed.work.lease.unwrap().claim;
    let sequence = agent.conversation.last_seq + 1;
    let turn = sequence + 1;
    let mut next = agent.clone();
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
    store
        .commit_agent_conversation(AgentConversationMutation {
            settings_fence: None,
            command_claim: Some(claim.clone()),
            execution_fence: None,
            trigger_execution_fence: None,
            admission_guard: Some(SandboxMutationGuard::capture(&parent)),
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
        })
        .await
        .unwrap();
    store
        .release_command_claim(&claim, layer2_core::now_rfc3339())
        .await
        .unwrap();
    let agent = store.get_agent(&agent.id).await.unwrap().unwrap();
    let source = store
        .get_command(&source.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    let mut command = live_request(&agent, &parent, "targeted-key");
    command.envelope.command = AgentCommand::Interrupt;
    command.envelope.request_fingerprint = command.envelope.command.fingerprint().unwrap();
    command.response_id = None;
    command.queue_capacity = 8;
    let request = TargetedControlReservation {
        command,
        target: TargetedResponse {
            response_id: source.receipt.response_id.unwrap(),
            turn_sequence: source.work.dispatch.unwrap().turn_sequence,
        },
        observed_agent_revision: agent.revision,
        agent_created_at: agent.created_at.clone(),
        retry_deadline: None,
    };
    (agent, request)
}
