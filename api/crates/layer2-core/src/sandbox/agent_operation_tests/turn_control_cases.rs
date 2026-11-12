use super::*;

#[tokio::test]
async fn steer_requires_working_status_an_active_turn_and_capability() {
    for status in ALL_AGENT_STATUSES {
        if status == AgentStatus::Working {
            continue;
        }
        let store = Arc::new(InMemorySandboxStore::default());
        let agent = test_agent(status, None);
        store.create_agent(agent.clone()).await.unwrap();
        let (sender, _) = recording_sender(Ok(()));
        let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
        let service = AgentOperationService::new(store, registry.clone());
        assert_eq!(
            service.steer(&owner(), &agent.id, "more".into()).await,
            Err(AgentOperationError::StatusConflict { status })
        );
        assert_eq!(registry.resolutions(), 0);
    }

    let store = Arc::new(InMemorySandboxStore::default());
    let mut agent = test_agent(AgentStatus::Working, None);
    agent.conversation.active_turn = None;
    store.create_agent(agent.clone()).await.unwrap();
    let (sender, _) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(store, registry.clone());
    assert_eq!(
        service.steer(&owner(), &agent.id, "more".into()).await,
        Err(AgentOperationError::NoActiveTurn)
    );
    assert_eq!(registry.resolutions(), 0);

    let store = Arc::new(InMemorySandboxStore::default());
    let mut agent = test_agent(AgentStatus::Working, None);
    agent.capabilities = RuntimeCapabilities::new([RuntimeCapability::Conversation]);
    store.create_agent(agent.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(store.clone(), registry.clone());
    assert_eq!(
        service.steer(&owner(), &agent.id, "more".into()).await,
        Err(AgentOperationError::CapabilityUnsupported {
            capability: RuntimeCapability::Steer
        })
    );
    assert_eq!(registry.resolutions(), 0);
    assert!(inputs.lock().unwrap().is_empty());
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));

    let store = Arc::new(InMemorySandboxStore::default());
    let agent = test_agent(AgentStatus::Working, None);
    seed_active_parent(&store).await;
    store.create_agent(agent.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(
        RuntimeCapabilities::new([RuntimeCapability::Conversation]),
        sender,
    );
    let service = AgentOperationService::new(store.clone(), registry);
    assert_eq!(
        service.steer(&owner(), &agent.id, "more".into()).await,
        Err(AgentOperationError::CapabilityUnsupported {
            capability: RuntimeCapability::Steer
        })
    );
    assert!(inputs.lock().unwrap().is_empty());
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
}

#[tokio::test]
async fn steer_and_interrupt_send_the_expected_runtime_inputs() {
    let steer_store = Arc::new(InMemorySandboxStore::default());
    seed_active_parent(&steer_store).await;
    let working = test_agent(AgentStatus::Working, None);
    steer_store.create_agent(working.clone()).await.unwrap();
    let (sender, steer_inputs) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        steer_store,
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let receipt = service
        .steer(&owner(), &working.id, "Narrow the search.".into())
        .await
        .unwrap();
    assert_eq!(receipt.turn_sequence, ACTIVE_TURN);
    assert!(matches!(
        steer_inputs.lock().unwrap().as_slice(),
        [ConversationInput::Steer(content)] if prompt_text(content) == "Narrow the search."
    ));

    let interrupt_store = Arc::new(InMemorySandboxStore::default());
    seed_active_parent(&interrupt_store).await;
    interrupt_store.create_agent(working.clone()).await.unwrap();
    let (sender, interrupt_inputs) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        interrupt_store,
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    service.interrupt(&owner(), &working.id).await.unwrap();
    assert!(matches!(
        interrupt_inputs.lock().unwrap().as_slice(),
        [ConversationInput::Cancel]
    ));
}

#[tokio::test]
async fn interrupt_accepts_only_active_working_or_waiting_agents() {
    for status in ALL_AGENT_STATUSES {
        if matches!(status, AgentStatus::Working | AgentStatus::WaitingApproval) {
            continue;
        }
        let store = Arc::new(InMemorySandboxStore::default());
        let agent = test_agent(status, None);
        store.create_agent(agent.clone()).await.unwrap();
        let (sender, inputs) = recording_sender(Ok(()));
        let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
        let service = AgentOperationService::new(store, registry.clone());
        assert_eq!(
            service.interrupt(&owner(), &agent.id).await,
            Err(AgentOperationError::StatusConflict { status })
        );
        assert_eq!(registry.resolutions(), 0);
        assert!(inputs.lock().unwrap().is_empty());
    }

    for status in [AgentStatus::Working, AgentStatus::WaitingApproval] {
        let store = Arc::new(InMemorySandboxStore::default());
        let mut agent = test_agent(status, None);
        agent.conversation.active_turn = None;
        store.create_agent(agent.clone()).await.unwrap();
        let (sender, inputs) = recording_sender(Ok(()));
        let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
        let service = AgentOperationService::new(store, registry.clone());
        assert_eq!(
            service.interrupt(&owner(), &agent.id).await,
            Err(AgentOperationError::NoActiveTurn)
        );
        assert_eq!(registry.resolutions(), 0);
        assert!(inputs.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn interrupt_waiting_for_approval_can_complete_immediately() {
    let store = Arc::new(InMemorySandboxStore::default());
    let waiting = seed_waiting_approval(&store).await;
    let sender = completing_sender(store.clone(), waiting.id.clone(), true);
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );

    let receipt = service.interrupt(&owner(), &waiting.id).await.unwrap();

    assert_eq!(receipt.delivery, AgentOperationDelivery::ExecutionObserved);
    assert_eq!(receipt.agent.status, AgentStatus::Ready);
    assert_eq!(receipt.agent.conversation.active_turn, None);
    assert!(store
        .agent_log(&waiting.id, 0, 30)
        .await
        .unwrap()
        .iter()
        .any(|entry| matches!(entry.kind, AgentLogKind::TurnInterrupted { .. })));
}

#[tokio::test]
async fn interrupt_pre_write_failure_restores_waiting_for_approval() {
    let store = Arc::new(InMemorySandboxStore::default());
    let waiting = seed_waiting_approval(&store).await;
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender).fail_on_resolution(2);
    let service = AgentOperationService::new(store.clone(), registry);

    assert!(matches!(
        service.interrupt(&owner(), &waiting.id).await,
        Err(AgentOperationError::RuntimeUnavailable(_))
    ));
    assert!(inputs.lock().unwrap().is_empty());
    let stored = store.get_agent(&waiting.id).await.unwrap().unwrap();
    assert_eq!(stored.status, AgentStatus::WaitingApproval);
    assert_eq!(stored.conversation.active_turn, Some(ACTIVE_TURN));
}
