use super::*;

#[test]
fn operation_record_distinguishes_every_delivery_boundary() {
    let mut confirmed = operation_record(AgentOperationIntent::send_message("hello"));
    assert!(confirmed.blocks_dispatch());
    confirmed.mark_write_confirmed(LATER.into()).unwrap();
    assert_eq!(confirmed.state, AgentOperationDelivery::WriteConfirmed);
    assert!(!confirmed.blocks_dispatch());
    confirmed.mark_execution_observed(LATEST.into()).unwrap();
    assert_eq!(confirmed.state, AgentOperationDelivery::ExecutionObserved);

    let failure = AgentOperationFailure::new("not_written", "writer absent", LATER).unwrap();
    let mut pre_write = operation_record(AgentOperationIntent::Interrupt);
    pre_write.mark_pre_write_failed(failure).unwrap();
    assert_eq!(pre_write.state, AgentOperationDelivery::PreWriteFailed);
    assert!(!pre_write.blocks_dispatch());

    let failure = AgentOperationFailure::new("ambiguous", "receipt lost", LATER).unwrap();
    let mut ambiguous = operation_record(AgentOperationIntent::steer("more"));
    ambiguous.require_recovery(failure).unwrap();
    assert_eq!(ambiguous.state, AgentOperationDelivery::RecoveryRequired);
    assert!(ambiguous.blocks_dispatch());
    ambiguous.mark_execution_observed(LATEST.into()).unwrap();
    assert_eq!(ambiguous.state, AgentOperationDelivery::ExecutionObserved);
    assert!(ambiguous.failure.is_none());
    assert!(ambiguous.failed_at.is_none());

    let mut corrupt = operation_record(AgentOperationIntent::Interrupt);
    corrupt
        .mark_pre_write_failed(
            AgentOperationFailure::new("not_written", "writer absent", LATER).unwrap(),
        )
        .unwrap();
    corrupt.write_confirmed_at = Some(LATEST.into());
    assert!(corrupt.validate().is_err());
}

#[test]
fn queued_message_transitions_preserve_the_retry_safe_boundary() {
    let mut message = QueuedAgentMessage::new(1, "hello".into(), NOW.into()).unwrap();
    message.fence_dispatch(LATER.into()).unwrap();
    assert_eq!(message.state, QueuedMessageState::DispatchFenced);
    message.retry_after_pre_write_failure().unwrap();
    assert_eq!(message.state, QueuedMessageState::Queued);
    assert!(message.dispatch_fenced_at.is_none());

    message.fence_dispatch(LATER.into()).unwrap();
    let mut delivered = message.clone();
    delivered.mark_delivered().unwrap();
    assert_eq!(delivered.state, QueuedMessageState::Delivered);
    message.require_recovery().unwrap();
    assert_eq!(message.state, QueuedMessageState::RecoveryRequired);
    assert!(message.retry_after_pre_write_failure().is_err());
}

#[tokio::test]
async fn send_message_fences_before_input_and_moves_ready_to_working() {
    let store = Arc::new(InMemorySandboxStore::default());
    let ready = test_agent(AgentStatus::Ready, None);
    seed_active_parent(&store).await;
    store.create_agent(ready.clone()).await.unwrap();
    let fence_seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let inputs = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sender = fence_checking_sender(
        store.clone(),
        ready.id.clone(),
        fence_seen.clone(),
        inputs.clone(),
    );
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(store.clone(), registry.clone());

    let outcome = service
        .send_message(&owner(), &ready.id, "Inspect this.".into())
        .await
        .unwrap();
    let AgentMessageOutcome::Dispatched(receipt) = outcome else {
        panic!("ready input must dispatch")
    };

    assert!(fence_seen.load(Ordering::SeqCst));
    assert_eq!(receipt.agent.status, AgentStatus::Working);
    assert_eq!(
        receipt.agent.conversation.active_turn,
        Some(receipt.turn_sequence)
    );
    assert_eq!(receipt.delivery, AgentOperationDelivery::WriteConfirmed);
    assert!(matches!(
        inputs.lock().unwrap().as_slice(),
        [ConversationInput::RecoverablePrompt { content, prompt_id }]
            if prompt_text(content) == "Inspect this."
                && prompt_id == &layer3_adapter::PromptIdentity::new(
                    receipt.agent.conversation.conversation_id.as_deref().unwrap(),
                    receipt.agent.conversation.runtime_generation.unwrap(),
                    receipt.turn_sequence,
                )
    ));
    let logs = store.agent_log(&ready.id, 0, 20).await.unwrap();
    let logs: Vec<_> = logs
        .into_iter()
        .filter(|entry| !matches!(entry.kind, AgentLogKind::RuntimeMetadataChanged { .. }))
        .collect();
    assert!(matches!(
        logs.as_slice(),
        [
            AgentLogEntry {
                kind: AgentLogKind::OperationDeliveryChanged {
                    state: AgentOperationDelivery::DispatchFenced,
                    ..
                },
                ..
            },
            AgentLogEntry {
                kind: AgentLogKind::TurnStarted { .. },
                ..
            },
            AgentLogEntry {
                kind: AgentLogKind::OwnerMessage { .. },
                ..
            },
            AgentLogEntry {
                kind: AgentLogKind::StatusChanged {
                    from: AgentStatus::Ready,
                    to: AgentStatus::Working
                },
                ..
            },
            AgentLogEntry {
                kind: AgentLogKind::OperationDeliveryChanged {
                    state: AgentOperationDelivery::WriteConfirmed,
                    ..
                },
                ..
            },
        ]
    ));
    assert_eq!(registry.resolutions(), 2);
}

#[tokio::test]
async fn pre_write_failure_rolls_a_new_turn_back_to_ready() {
    let store = Arc::new(InMemorySandboxStore::default());
    let ready = test_agent(AgentStatus::Ready, None);
    seed_active_parent(&store).await;
    store.create_agent(ready.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender).fail_on_resolution(2);
    let service = AgentOperationService::new(store.clone(), registry);

    let error = service
        .send_message(&owner(), &ready.id, "Inspect this.".into())
        .await
        .unwrap_err();

    assert!(matches!(error, AgentOperationError::RuntimeUnavailable(_)));
    assert!(inputs.lock().unwrap().is_empty());
    let stored = store.get_agent(&ready.id).await.unwrap().unwrap();
    assert_eq!(stored.status, AgentStatus::Ready);
    assert_eq!(stored.conversation.active_turn, None);
    assert_eq!(
        stored.conversation.operation.unwrap().state,
        AgentOperationDelivery::PreWriteFailed
    );
    assert!(store
        .agent_log(&ready.id, 0, 20)
        .await
        .unwrap()
        .iter()
        .any(|entry| matches!(entry.kind, AgentLogKind::TurnFailed { .. })));
}

#[tokio::test]
async fn ambiguous_delivery_is_durable_and_blocks_a_repeat() {
    let store = Arc::new(InMemorySandboxStore::default());
    let ready = test_agent(AgentStatus::Ready, None);
    seed_active_parent(&store).await;
    store.create_agent(ready.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Err(layer3_adapter::AdapterError::Rejected(
        "receipt disappeared".into(),
    )));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(store.clone(), registry.clone());

    let error = service
        .send_message(&owner(), &ready.id, "Inspect this.".into())
        .await
        .unwrap_err();
    let sequence = match error {
        AgentOperationError::DeliveryAmbiguous {
            operation_sequence, ..
        } => operation_sequence,
        other => panic!("unexpected error: {other:?}"),
    };
    assert_eq!(inputs.lock().unwrap().len(), 1);
    let stored = store.get_agent(&ready.id).await.unwrap().unwrap();
    assert_eq!(
        stored.conversation.operation.as_ref().unwrap().state,
        AgentOperationDelivery::RecoveryRequired
    );

    assert_eq!(
        service
            .steer(&owner(), &ready.id, "Do it again.".into())
            .await,
        Err(AgentOperationError::DispatchRecoveryRequired {
            operation_sequence: sequence
        })
    );
    assert_eq!(registry.resolutions(), 2);
    assert_eq!(inputs.lock().unwrap().len(), 1);

    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    let mut assembly = AgentMessageAssembly::default();
    recorder
        .record_update(
            &ready.id,
            3,
            &mut assembly,
            layer3_adapter::ConversationUpdate::TurnEnded(layer3_adapter::PromptResponse::new(
                layer3_adapter::TurnStopReason::EndTurn,
            )),
            LATEST.into(),
        )
        .await
        .unwrap();
    let recovered = store.get_agent(&ready.id).await.unwrap().unwrap();
    assert_eq!(recovered.status, AgentStatus::Ready);
    assert_eq!(
        recovered.conversation.operation.unwrap().state,
        AgentOperationDelivery::ExecutionObserved
    );
}

#[tokio::test]
async fn send_message_accepts_only_ready_or_paused_status() {
    for status in ALL_AGENT_STATUSES {
        if matches!(status, AgentStatus::Ready | AgentStatus::Paused) {
            continue;
        }
        let store = Arc::new(InMemorySandboxStore::default());
        let agent = test_agent(status, None);
        store.create_agent(agent.clone()).await.unwrap();
        let (sender, inputs) = recording_sender(Ok(()));
        let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
        let service = AgentOperationService::new(store, registry.clone());
        assert_eq!(
            service
                .send_message(&owner(), &agent.id, "hello".into())
                .await,
            Err(AgentOperationError::StatusConflict { status })
        );
        assert_eq!(registry.resolutions(), 0);
        assert!(inputs.lock().unwrap().is_empty());
    }

    let store = Arc::new(InMemorySandboxStore::default());
    let mut corrupt = test_agent(AgentStatus::Ready, None);
    corrupt.conversation.active_turn = Some(ACTIVE_TURN);
    store.create_agent(corrupt.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(store, registry.clone());
    assert!(matches!(
        service
            .send_message(&owner(), &corrupt.id, "hello".into())
            .await,
        Err(AgentOperationError::Corrupt(_))
    ));
    assert_eq!(registry.resolutions(), 0);
    assert!(inputs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn stop_intent_rejects_new_input_before_runtime_resolution() {
    let store = Arc::new(InMemorySandboxStore::default());
    let mut stopping = test_agent(AgentStatus::Ready, None);
    store.create_agent(stopping.clone()).await.unwrap();
    let expected = stopping.revision;
    stopping
        .request_stop(AgentStopReason::Owner, LATER.into())
        .unwrap();
    store.save_agent(expected, stopping.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(store, registry.clone());

    assert_eq!(
        service
            .send_message(&owner(), &stopping.id, "too late".into())
            .await,
        Err(AgentOperationError::StatusConflict {
            status: AgentStatus::Ready
        })
    );
    assert_eq!(registry.resolutions(), 0);
    assert!(inputs.lock().unwrap().is_empty());
}
