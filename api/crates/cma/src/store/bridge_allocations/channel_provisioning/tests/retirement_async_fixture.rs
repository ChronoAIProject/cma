use super::super::super::lifecycle_tests::{
    command_fixture::QueuedChannelFixture, lifecycle_fixture::LifecycleFixture,
};
use super::*;
use layer2_core::sandbox::{AgentLogEntry, AgentOperationService};

pub(super) async fn dispatch(
    lifecycle: &LifecycleFixture,
    message: &str,
) -> (QueuedChannelFixture, AdmittedAgentCommand) {
    let queued = lifecycle.queued_channel_command(message).await;
    let now = SystemTime::now();
    let claimed = lifecycle
        .f
        .original
        .resources
        .claim_next_command(CommandClaimRequest {
            agent_id: queued.command.receipt.envelope.agent_id.clone(),
            holder: "retirement-original-command".into(),
            now: layer2_core::rfc3339_at(now),
            expires_at: layer2_core::rfc3339_at(now + Duration::from_secs(30)),
        })
        .await
        .unwrap()
        .unwrap();
    let authority = queued
        .channel
        .authorize_execution(&queued.invokes, &claimed)
        .await
        .unwrap();
    AgentOperationService::new(
        lifecycle.f.original.resources.clone(),
        lifecycle.registry.clone(),
    )
    .execute_channel_claim(&authority, &claimed)
    .await
    .unwrap();
    let dispatched = lifecycle
        .f
        .original
        .resources
        .get_command(&claimed.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(dispatched.status, AgentResponseStatus::InProgress);
    assert_eq!(dispatched.receipt, queued.command.receipt);
    assert!(dispatched.work.dispatch.is_some());
    (queued, dispatched)
}

pub(super) async fn finish_turn(lifecycle: &LifecycleFixture, command: &AdmittedAgentCommand) {
    use layer3_adapter::{ConversationUpdate, PromptResponse, TurnStopReason};
    let runtime = layer3_adapter::RuntimeHandle(
        lifecycle
            .active
            .runtime
            .as_ref()
            .unwrap()
            .runtime_id
            .clone(),
    );
    let agent = &command.receipt.envelope.agent_id;
    lifecycle
        .f
        .original
        .backend
        .send_agent_update(
            &runtime,
            &layer3_adapter::AgentSlot(agent.to_string()),
            ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn)),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if lifecycle
                .f
                .original
                .resources
                .get_agent(agent)
                .await
                .unwrap()
                .unwrap()
                .conversation
                .active_turn
                .is_none()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let turn = command.work.dispatch.as_ref().unwrap().turn_sequence;
    let logs = lifecycle
        .f
        .original
        .resources
        .agent_log(agent, 0, 100)
        .await
        .unwrap();
    assert_eq!(
        logs.iter()
            .filter(|entry| matches!(entry.kind,
        AgentLogKind::TurnCompleted { turn_sequence } if turn_sequence == turn))
            .count(),
        1
    );
    // A completed durable turn precedes ledger recovery: this worker must read
    // its original history after restart, rather than infer success from stop.
    let retained = lifecycle
        .f
        .original
        .resources
        .get_command(&command.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.status, AgentResponseStatus::InProgress);
    assert_eq!(retained.receipt, command.receipt);
}

pub(super) async fn close_requests(
    f: &RetirementFixture,
) -> Vec<layer3_adapter::wire::FencedRequest<layer3_adapter::AgentCloseRequest>> {
    f.f.original
        .backend
        .requests()
        .await
        .into_iter()
        .filter_map(|(_, frame)| match frame {
            layer3_adapter::wire::RequestFrame::CloseAgent(request) => Some(request),
            _ => None,
        })
        .collect()
}

pub(super) async fn await_agent_work_due(
    resources: &crate::store::MongoSandboxStore,
    id: &layer2_core::AgentId,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let agent = resources.get_agent(id).await.unwrap().unwrap();
            if agent.work.is_due(&layer2_core::now_rfc3339()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the retained close intent must become due for its replacement worker");
}

pub(super) async fn log(f: &RetirementFixture) -> Vec<AgentLogEntry> {
    f.f.original
        .resources
        .agent_log(&f.f.original.record.placement.target().agent_id, 0, 100)
        .await
        .unwrap()
}

pub(super) struct NoDispatch;
#[async_trait::async_trait]
impl AgentCommandExecution for NoDispatch {
    async fn release_held(&self, _: &AdmittedAgentCommand) {
        panic!("retirement recovery released a new command");
    }
    async fn check_queued(&self, _: &AdmittedAgentCommand) -> CommandReadiness {
        panic!("retirement recovery checked a fresh dispatch");
    }
    async fn prepare_queued(&self, _: &AdmittedAgentCommand) {
        panic!("retirement recovery prepared a new command");
    }
    async fn execute(&self, _: &AdmittedAgentCommand) -> CommandAttempt {
        panic!("retirement recovery sent a second prompt");
    }
}

pub(super) async fn source_records(
    f: &RetirementFixture,
    command: &AdmittedAgentCommand,
) -> Document {
    let row = f
        .db()
        .collection::<Document>("agent_commands")
        .find_one(doc! {"_id":command.receipt.envelope.command_id.as_str()})
        .await
        .unwrap()
        .unwrap();
    doc! {
        "channel_admission":row.get("channel_admission").unwrap().clone(),
        "channel_source":row.get("channel_source").unwrap().clone(),
    }
}
