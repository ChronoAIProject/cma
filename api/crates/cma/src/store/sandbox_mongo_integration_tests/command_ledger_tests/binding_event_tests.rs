use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn external_event_context_commits_with_admission_and_survives_restart_and_dispatch() {
    let (client, name) = database("binding_event_context").await;
    let (store, agent, parent, execution, _, sends) = composed(&client, &name).await;
    let (binding, conversation, _) = binding_for(&store, &agent).await;
    let mut first = reservation(&store, &agent, &parent, &binding).await;
    let AgentCommand::Message { input, .. } = &mut first.envelope.command else {
        unreachable!()
    };
    input.content.push(AgentContent::ExternalEvent {
        context: ExternalEventContext {
            provider_conversation_id: None,
            source_id: "independent-channel".into(),
            conversation_id: conversation.conversation_id.clone(),
            event_id: "external-event".into(),
            reference: "sealed-reference".into(),
            payload: serde_json::json!({"nested":{"keep":true},"body":"x".repeat(300_000)}),
        },
    });
    first.envelope.request_fingerprint = first.envelope.command.fingerprint().unwrap();
    let key = first.envelope.external_event_key().unwrap();
    assert!(store
        .get_external_event_command(&key)
        .await
        .unwrap()
        .is_none());
    let mut invalid = first.clone();
    invalid.expected_agent_revision += 1;
    assert_eq!(
        store.reserve_command(invalid).await,
        Err(CommandLedgerError::Conflict)
    );
    assert!(store
        .get_external_event_command(&key)
        .await
        .unwrap()
        .is_none());
    let mut second = first.clone();
    second.key_sha256 = command_key_digest("other-event-key").unwrap();
    second.envelope.command_id = AgentCommandId::generate();
    second.response_id = Some(AgentResponseId::generate());
    let (one, two) = tokio::join!(
        store.reserve_command(first.clone()),
        store.reserve_command(second.clone())
    );
    assert_eq!(usize::from(one.is_ok()) + usize::from(two.is_ok()), 1);
    let (original, retry, error) = match one {
        Ok(result) => (record(result), first, two.unwrap_err()),
        Err(error) => (record(two.unwrap()), second, error),
    };
    assert_eq!(error, CommandLedgerError::IdempotencyConflict);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened.get_external_event_command(&key).await.unwrap(),
        Some(original.clone())
    );
    assert_eq!(
        reopened.reserve_command(retry.clone()).await.unwrap(),
        CommandReservationResult::Existing(Box::new(original.clone()))
    );
    let mut changed = retry;
    let AgentCommand::Message { input, .. } = &mut changed.envelope.command else {
        unreachable!()
    };
    let AgentContent::ExternalEvent { context } = &mut input.content[1] else {
        unreachable!()
    };
    context.payload["nested"]["keep"] = false.into();
    changed.envelope.request_fingerprint = changed.envelope.command.fingerprint().unwrap();
    assert_eq!(
        reopened.reserve_command(changed).await,
        Err(CommandLedgerError::IdempotencyConflict)
    );
    let command = store
        .claim_next_command(claim_request(
            &agent,
            "binding-worker",
            &layer2_core::now_rfc3339(),
        ))
        .await
        .unwrap()
        .unwrap();
    let auth = auth(store.clone());
    let access = ResourceAccessService::new(store.clone(), auth.clone())
        .with_bindings(BindingAccessService::new(store.clone()));
    let proof = auth
        .authorize_resource_command(&access, &command.receipt.envelope)
        .await
        .unwrap();
    execution
        .operations
        .execute_resource_claim(&proof, &command)
        .await
        .unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let saved = reopened
        .get_external_event_command(&key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.receipt, original.receipt);
    assert!(saved.work.dispatch.is_some());
    client.database(&name).drop().await.unwrap();
}
