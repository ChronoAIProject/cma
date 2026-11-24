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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn queued_input_stale_observation_aborts_without_changing_source_or_receipt() {
    let (client, name) = database("queued_input_stale").await;
    let (store, mut agent, _, mut request, source) = queued(&name, &client).await;
    let old_revision = agent.revision;
    agent.revision += 1;
    store.save_agent(old_revision, agent.clone()).await.unwrap();

    assert_eq!(
        store.edit_queued_input(request.clone()).await.unwrap(),
        QueuedInputOutcome::StaleObservation
    );
    assert_eq!(
        store
            .get_command(&request.cancellation.target.source_command_id)
            .await
            .unwrap(),
        Some(source.clone())
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        1
    );

    request.cancellation.command.expected_agent_revision = agent.revision;
    request.cancellation.observed_agent_revision = agent.revision;
    let applied = applied(store.edit_queued_input(request.clone()).await.unwrap());
    assert_eq!(applied.response.revision, source.revision + 1);
    assert_eq!(applied.response.queued_input.unwrap().text, request.text);
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        2
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn queued_edit_and_removal_fence_supported_sandbox_work_envelopes() {
    use layer2_core::sandbox::store::WorkAuthorityProtocol;

    for (proof, version) in [(None, 3_i64), (Some("a".repeat(64)), 4_i64)] {
        let (client, name) = database("queued_input_work_envelope").await;
        let (store, agent, mut parent, mut edit, source) = queued(&name, &client).await;
        let prior_revision = parent.revision;
        parent.revision += 1;
        parent.work.authority_protocol = Some(WorkAuthorityProtocol::RenewedV1);
        parent.work.prepared_installation_digest = proof;
        store
            .save_sandbox(prior_revision, parent.clone())
            .await
            .unwrap();

        let sandbox_document = || store.sandboxes.find_one(doc! {"_id": parent.id.as_str()});
        let mut before = sandbox_document().await.unwrap().unwrap();
        let before_edit_fence = before.get_i64("agent_command_admission_fence").unwrap_or(0);
        assert_eq!(before.get_i64("document_schema_version").unwrap(), version);
        edit.cancellation.command.parent = SandboxMutationGuard::capture(&parent);

        let saved = applied(store.edit_queued_input(edit.clone()).await.unwrap());
        assert_eq!(saved.response.receipt, source.receipt);
        assert_eq!(saved.response.revision, source.revision + 1);
        assert_eq!(saved.binding.text, edit.text);
        let after_edit = sandbox_document().await.unwrap().unwrap();
        assert_eq!(
            after_edit.get_i64("agent_command_admission_fence").unwrap(),
            before_edit_fence + 1
        );
        assert_eq!(
            applied(store.edit_queued_input(edit.clone()).await.unwrap()),
            saved
        );

        let mut remove = edit.cancellation;
        remove.command.key_sha256 = command_key_digest("remove-edited-input").unwrap();
        remove.command.envelope.command_id = AgentCommandId::generate();
        remove.target.expected_command_revision = saved.response.revision;
        let before_removal = sandbox_document().await.unwrap().unwrap();
        let result = store.cancel_queued_response(remove.clone()).await.unwrap();
        let QueuedCancellationOutcome::Applied(cancelled) = &result else {
            panic!("expected a cancellation receipt");
        };
        assert_eq!(cancelled.response.status, AgentResponseStatus::Cancelled);
        assert_eq!(cancelled.response.revision, source.revision + 2);
        let after_removal = sandbox_document().await.unwrap().unwrap();
        assert_eq!(
            after_removal
                .get_i64("agent_command_admission_fence")
                .unwrap(),
            before_removal
                .get_i64("agent_command_admission_fence")
                .unwrap()
                + 1
        );
        assert_eq!(store.cancel_queued_response(remove).await.unwrap(), result);
        assert!(store
            .list_queued_inputs(&agent.id)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            store.agent_commands.count_documents(doc! {}).await.unwrap(),
            3
        );

        let mut after = sandbox_document().await.unwrap().unwrap();
        before.remove("agent_command_admission_fence");
        after.remove("agent_command_admission_fence");
        assert_eq!(after, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[path = "queued_input_race_tests.rs"]
mod races;
