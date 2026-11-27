use super::*;

pub(super) fn confirmed_operation(agent: &Agent, turn: u64) -> AgentOperationRecord {
    let mut operation = AgentOperationRecord::new(
        1,
        agent.conversation.runtime_generation.unwrap(),
        turn,
        AgentOperationIntent::SendMessage {
            payload_sha256: "a".repeat(64),
        },
        NOW.into(),
    )
    .unwrap();
    operation.mark_write_confirmed(NOW.into()).unwrap();
    operation
}

pub(super) async fn observe_operation(store: &InMemorySandboxStore, progress: Progress) {
    let mut agent = store
        .get_agent(&AgentId("agent".into()))
        .await
        .unwrap()
        .unwrap();
    let operation = agent.conversation.operation.as_mut().unwrap();
    if matches!(progress, Progress::ReplacedDelivery) {
        operation.intent = AgentOperationIntent::SendMessage {
            payload_sha256: "b".repeat(64),
        };
    }
    operation.mark_execution_observed(NOW.into()).unwrap();
    let event = store
        .append_agent_log(
            &agent.id,
            NOW.into(),
            AgentLogKind::OperationDeliveryChanged {
                operation_sequence: operation.sequence,
                operation: AgentOperationKind::SendMessage,
                state: operation.state,
            },
        )
        .await
        .unwrap();
    agent.conversation.last_seq = event.seq;
    let revision = agent.revision;
    agent.revision += 1;
    store.save_agent(revision, agent).await.unwrap();
}

#[tokio::test]
async fn queued_message_refreshes_after_execution_is_observed_on_the_same_turn() {
    assert_message_refreshes_after_observation(Progress::ConversationDelivery, Some(2)).await;
}

#[tokio::test]
async fn queued_message_refuses_refresh_when_the_observed_payload_changed() {
    let (store, authority) = observation_fixture(Progress::ReplacedDelivery, Some(2)).await;
    let (service, _) = service(&store, authority, Progress::ReplacedDelivery);
    let owner = context("owner");
    let request = submission("message-during-observation");
    assert!(matches!(
        service
            .admit(
                &owner,
                "selected-credential",
                CommandSubmissionSource::Api,
                request.clone()
            )
            .await,
        Err(CommandAdmissionError::Ledger(CommandLedgerError::Conflict))
    ));
    assert_eq!(
        service
            .lookup_admitted(
                &owner,
                "selected-credential",
                CommandSubmissionSource::Api,
                request
            )
            .await
            .unwrap(),
        None
    );
}
