use super::super::work_tests::{claim_request, live_request, message_mutation};
use super::*;
use layer2_core::sandbox::{AgentMessageAssembly, AgentUpdateRecorder, DurableAgentUpdateRecorder};

pub(super) async fn composed(
    name: &str,
    client: &Client,
) -> (
    MongoSandboxStore,
    Agent,
    TargetedControlReservation,
    AdmittedAgentCommand,
) {
    let (store, mut agent, parent) = fixture(name, client).await;
    agent.capabilities = layer2_core::RuntimeCapabilities::new([
        layer2_core::RuntimeCapability::Conversation,
        layer2_core::RuntimeCapability::Interrupt,
        layer2_core::RuntimeCapability::Steer,
    ]);
    replace_agent_fixture(&store, &agent).await;
    let source = record(
        store
            .reserve_command(live_request(&agent, &parent, "source"))
            .await
            .unwrap(),
    );
    let claimed = store
        .claim_next_command(claim_request(
            &agent,
            "source-worker",
            &layer2_core::now_rfc3339(),
        ))
        .await
        .unwrap()
        .unwrap();
    let claim = claimed.work.lease.as_ref().unwrap().claim.clone();
    store
        .commit_agent_conversation(message_mutation(&agent, &parent, &claim))
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
    let mut command = live_request(&agent, &parent, "targeted");
    command.envelope.command = AgentCommand::Interrupt;
    command.envelope.request_fingerprint = command.envelope.command.fingerprint().unwrap();
    command.response_id = None;
    command.queue_capacity = 8;
    let mut audit = super::super::audit_tests::personal_audit(&agent, &parent);
    audit.capability = layer2_core::resource_access::ResourceCapability::OperateAgent;
    audit.observations[0].capabilities = vec![audit.capability];
    command.admission_audit = Some(audit);
    let request = TargetedControlReservation {
        command,
        target: TargetedResponse {
            response_id: source.receipt.response_id.clone().unwrap(),
            turn_sequence: source.work.dispatch.as_ref().unwrap().turn_sequence,
        },
        observed_agent_revision: agent.revision,
        agent_created_at: agent.created_at.clone(),
        retry_deadline: None,
    };
    (store, agent, request, source)
}

pub(super) async fn finish_and_start(
    store: &MongoSandboxStore,
    agent: &Agent,
    source: &AdmittedAgentCommand,
) -> Agent {
    DurableAgentUpdateRecorder::new(std::sync::Arc::new(store.clone()))
        .record_update(
            &agent.id,
            1,
            &mut AgentMessageAssembly::default(),
            layer3_adapter::ConversationUpdate::TurnEnded(layer3_adapter::PromptResponse::new(
                layer3_adapter::TurnStopReason::EndTurn,
            )),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    store
        .reconcile_command(&source.receipt.envelope.command_id)
        .await
        .unwrap();
    let ready = store.get_agent(&agent.id).await.unwrap().unwrap();
    let next = begin_stored_conversation_turn(store, ready).await;
    store.commit_agent_conversation(next).await.unwrap();
    store.get_agent(&agent.id).await.unwrap().unwrap()
}

pub(super) async fn snapshot(store: &MongoSandboxStore) -> Vec<Vec<mongodb::bson::Document>> {
    let mut result = Vec::new();
    for collection in [
        &store.agents,
        &store.sandboxes,
        &store.agent_commands,
        &store.agent_command_orders,
        &store.command_authorization_audits,
    ] {
        result.push(
            collection
                .find(doc! {})
                .sort(doc! { "_id": 1 })
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap(),
        );
    }
    result
}
