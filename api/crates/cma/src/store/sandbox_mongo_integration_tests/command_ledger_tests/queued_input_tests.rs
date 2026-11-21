use super::work_tests::{claim_request, message_mutation};
use super::*;
async fn queued(
    name: &str,
    client: &Client,
) -> (
    MongoSandboxStore,
    Agent,
    Sandbox,
    QueuedInputReservation,
    AdmittedAgentCommand,
) {
    let (store, agent, parent, cancellation, source) =
        super::queued_cancellation_tests::queued(name, client).await;
    (
        store,
        agent,
        parent,
        QueuedInputReservation {
            cancellation,
            text: "Revised message with attachment text".into(),
        },
        source,
    )
}
fn applied(value: QueuedInputOutcome) -> AdmittedQueuedInput {
    match value {
        QueuedInputOutcome::Applied(value) => *value,
        other => panic!("expected edit, got {other:?}"),
    }
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn queued_input_mongo_replicas_recover_one_revision_and_dispatch_only_revised_text() {
    let (client, name) = database("queued_input_revision").await;
    let (store, agent, parent, request, source) = queued(&name, &client).await;
    let replica = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (one, two) = tokio::join!(
        store.edit_queued_input(request.clone()),
        replica.edit_queued_input(request.clone())
    );
    let original = applied(one.unwrap());
    assert_eq!(original, applied(two.unwrap()));
    assert_eq!(original.response.receipt, source.receipt);
    assert_eq!(
        replica.list_queued_inputs(&agent.id).await.unwrap(),
        vec![original.response.clone()]
    );
    let raw = store
        .agent_commands
        .find_one(doc! {"_id":source.receipt.envelope.command_id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(
        mongodb::bson::deserialize_from_bson::<AdmittedAgentCommand>(
            raw.get("record").unwrap().clone()
        )
        .is_err(),
        "older strict readers must refuse edited pending work"
    );
    let claimed = replica
        .claim_next_command(claim_request(
            &agent,
            "revised-worker",
            &layer2_core::now_rfc3339(),
        ))
        .await
        .unwrap()
        .unwrap();
    let claim = &claimed.work.lease.as_ref().unwrap().claim;
    let original_mutation = message_mutation(&agent, &parent, claim);
    assert!(matches!(
        replica
            .commit_agent_conversation(original_mutation.clone())
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    let mut revised = original_mutation;
    revised
        .agent
        .conversation
        .operation
        .as_mut()
        .unwrap()
        .intent = layer2_core::sandbox::AgentOperationIntent::send_message(&request.text);
    revised.agent_logs[2] = AgentLogKind::OwnerMessage {
        content: request.text.clone(),
    };
    replica.commit_agent_conversation(revised).await.unwrap();
    assert!(store
        .list_queued_inputs(&agent.id)
        .await
        .unwrap()
        .is_empty());
    let mut current_request = request.clone();
    current_request.cancellation.observed_agent_revision =
        store.get_agent(&agent.id).await.unwrap().unwrap().revision;
    let recovered = applied(store.edit_queued_input(current_request).await.unwrap());
    assert_eq!(recovered.binding, original.binding);
    assert_eq!(recovered.response.status, AgentResponseStatus::InProgress);
    let raw = store
        .agent_commands
        .find_one(doc! {"_id":source.receipt.envelope.command_id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(
        mongodb::bson::deserialize_from_bson::<AdmittedAgentCommand>(
            raw.get("record").unwrap().clone()
        )
        .is_ok(),
        "dispatch removes the pending compatibility guard"
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        2
    );
    client.database(&name).drop().await.unwrap();
}

#[path = "queued_input_race_tests.rs"]
mod races;
