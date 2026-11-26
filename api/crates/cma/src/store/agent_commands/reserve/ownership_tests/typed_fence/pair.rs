use super::*;
use layer2_core::ids::{AgentCommandId, AgentResponseId};
use layer2_core::sandbox::{
    AgentLogKind, AgentOperationDelivery, AgentOperationIntent, AgentOperationKind,
    AgentOperationRecord, AgentStatus,
};
pub(super) async fn prepare_pair(
    store: &MongoSandboxStore,
    agent: &Agent,
    parent: &Sandbox,
) -> InterruptingMessageReservation {
    let source = record(
        store
            .reserve_command(live_request(agent, parent, "original-message"))
            .await
            .unwrap(),
    );
    let now = std::time::SystemTime::now();
    let claimed = store
        .claim_next_command(CommandClaimRequest {
            agent_id: agent.id.clone(),
            holder: "pair-fixture".into(),
            now: layer2_core::rfc3339_at(now),
            expires_at: layer2_core::rfc3339_at(now + Duration::from_secs(30)),
        })
        .await
        .unwrap()
        .unwrap();
    let claim = claimed.work.lease.as_ref().unwrap().claim.clone();
    let at = layer2_core::now_rfc3339();
    let mut working = agent.clone();
    let sequence = working.conversation.last_seq + 1;
    let turn = sequence + 1;
    working
        .transition(AgentStatus::Working, at.clone(), None)
        .unwrap();
    working.conversation.active_turn = Some(turn);
    working.conversation.operation = Some(
        AgentOperationRecord::new(
            sequence,
            1,
            turn,
            AgentOperationIntent::send_message("Inspect this."),
            at.clone(),
        )
        .unwrap(),
    );
    working.conversation.last_seq += 4;
    store
        .commit_agent_conversation(AgentConversationMutation {
            settings_fence: None,
            command_claim: Some(claim.clone()),
            execution_fence: None,
            trigger_execution_fence: None,
            admission_guard: Some(SandboxMutationGuard::capture(parent)),
            expected_agent_revision: agent.revision,
            expected_last_seq: agent.conversation.last_seq,
            runtime_generation: 1,
            agent: working,
            appended_at: at,
            agent_logs: vec![
                AgentLogKind::OperationDeliveryChanged {
                    operation_sequence: sequence,
                    operation: AgentOperationKind::SendMessage,
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
    let current = store.get_agent(&agent.id).await.unwrap().unwrap();
    let mut command = live_request(&current, parent, "paired-message");
    command.envelope.command = AgentCommand::Interrupt;
    command.envelope.request_fingerprint = command.envelope.command.fingerprint().unwrap();
    command.response_id = None;
    command.queue_capacity = 8;
    let mut message = command.clone();
    message.envelope.command_id = AgentCommandId::generate();
    message.response_id = Some(AgentResponseId::generate());
    message.envelope.command = AgentCommand::Message {
        input: AgentInput::text("Replacement input"),
        order: AgentInputOrder::InterruptAfterTurn {
            turn_sequence: turn,
        },
        metadata: Default::default(),
    };
    message.envelope.request_fingerprint = message.envelope.command.fingerprint().unwrap();
    let request = InterruptingMessageReservation {
        control: TargetedControlReservation {
            command,
            target: TargetedResponse {
                response_id: source.receipt.response_id.unwrap(),
                turn_sequence: turn,
            },
            observed_agent_revision: current.revision,
            agent_created_at: current.created_at,
            retry_deadline: None,
        },
        message,
    };
    request.validate().unwrap();
    request
}
