use super::*;

#[tokio::test]
async fn paused_messages_are_bounded_without_runtime_side_effects() {
    let store = Arc::new(InMemorySandboxStore::default());
    seed_active_parent(&store).await;
    let paused = test_agent(AgentStatus::Paused, None);
    store.create_agent(paused.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(store.clone(), registry.clone());

    for index in 0..MAX_QUEUED_AGENT_MESSAGES {
        let outcome = service
            .send_message(&owner(), &paused.id, format!("message {index}"))
            .await
            .unwrap();
        assert!(matches!(outcome, AgentMessageOutcome::Queued { .. }));
    }
    assert_eq!(
        service
            .send_message(&owner(), &paused.id, "one too many".into())
            .await,
        Err(AgentOperationError::QueueCapacityExceeded {
            limit: MAX_QUEUED_AGENT_MESSAGES
        })
    );
    let stored = store.get_agent(&paused.id).await.unwrap().unwrap();
    assert_eq!(
        stored.conversation.queued_messages.len(),
        MAX_QUEUED_AGENT_MESSAGES
    );
    assert_eq!(registry.resolutions(), 0);
    assert!(inputs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn message_byte_bound_is_inclusive_and_checked_before_lookup() {
    let store = Arc::new(InMemorySandboxStore::default());
    seed_active_parent(&store).await;
    let paused = test_agent(AgentStatus::Paused, None);
    store.create_agent(paused.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(store.clone(), registry.clone());

    service
        .send_message(&owner(), &paused.id, "x".repeat(MAX_AGENT_MESSAGE_BYTES))
        .await
        .unwrap();
    let revision = store.get_agent(&paused.id).await.unwrap().unwrap().revision;
    assert!(matches!(
        service
            .send_message(
                &owner(),
                &paused.id,
                "x".repeat(MAX_AGENT_MESSAGE_BYTES + 1),
            )
            .await,
        Err(AgentOperationError::InvalidRequest(_))
    ));
    assert_eq!(
        store.get_agent(&paused.id).await.unwrap().unwrap().revision,
        revision
    );
    assert_eq!(registry.resolutions(), 0);
    assert!(inputs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn invalid_or_unsupported_input_reaches_neither_store_nor_sender() {
    let store = Arc::new(InMemorySandboxStore::default());
    let ready = test_agent(AgentStatus::Ready, None);
    seed_active_parent(&store).await;
    store.create_agent(ready.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(RuntimeCapabilities::default(), sender);
    let service = AgentOperationService::new(store.clone(), registry.clone());

    assert!(matches!(
        service
            .send_message(&owner(), &ready.id, "hello".into())
            .await,
        Err(AgentOperationError::CapabilityUnsupported {
            capability: RuntimeCapability::Conversation
        })
    ));
    assert_eq!(
        store.get_agent(&ready.id).await.unwrap(),
        Some(ready.clone())
    );
    assert!(store.agent_log(&ready.id, 0, 10).await.unwrap().is_empty());
    assert!(inputs.lock().unwrap().is_empty());

    assert!(matches!(
        service.send_message(&owner(), &ready.id, " ".into()).await,
        Err(AgentOperationError::InvalidRequest(_))
    ));
    assert_eq!(registry.resolutions(), 1);
}

#[tokio::test]
async fn a_failed_fence_commit_never_reaches_the_runtime_sender() {
    let store = Arc::new(RejectingConversationStore::default());
    let ready = test_agent(AgentStatus::Ready, None);
    seed_active_parent(&store.inner).await;
    store.inner.create_agent(ready.clone()).await.unwrap();
    store.reject_commits.store(true, Ordering::SeqCst);
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(store.clone(), registry.clone());

    assert!(matches!(
        service
            .send_message(&owner(), &ready.id, "hello".into())
            .await,
        Err(AgentOperationError::Unavailable(_))
    ));
    assert_eq!(store.commit_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(registry.resolutions(), 1);
    assert!(inputs.lock().unwrap().is_empty());
    assert_eq!(store.inner.get_agent(&ready.id).await.unwrap(), Some(ready));
}

#[tokio::test]
async fn stronger_execution_evidence_wins_the_settlement_race() {
    let store = Arc::new(InMemorySandboxStore::default());
    let ready = test_agent(AgentStatus::Ready, None);
    seed_active_parent(&store).await;
    store.create_agent(ready.clone()).await.unwrap();
    let sender = completing_sender(store.clone(), ready.id.clone(), false);
    let service =
        AgentOperationService::new(store, TestRuntimeRegistry::new(full_capabilities(), sender));

    let outcome = service
        .send_message(&owner(), &ready.id, "finish now".into())
        .await
        .unwrap();
    let AgentMessageOutcome::Dispatched(receipt) = outcome else {
        panic!("message must dispatch")
    };
    assert_eq!(receipt.delivery, AgentOperationDelivery::ExecutionObserved);
    assert_eq!(receipt.agent.status, AgentStatus::Ready);
}

#[tokio::test]
async fn authorization_and_ownership_fail_closed() {
    let store = Arc::new(InMemorySandboxStore::default());
    let ready = test_agent(AgentStatus::Ready, None);
    store.create_agent(ready.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(store, registry.clone());

    assert_eq!(
        service
            .send_message(&unauthorized_owner(), &ready.id, "hello".into())
            .await,
        Err(AgentOperationError::Forbidden)
    );
    assert_eq!(
        service
            .send_message(&other_owner(), &ready.id, "hello".into())
            .await,
        Err(AgentOperationError::NotFound)
    );
    assert_eq!(registry.resolutions(), 0);
    assert!(inputs.lock().unwrap().is_empty());
}
