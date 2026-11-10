mod admission_fence_tests;
mod command_authorization_tests;
mod command_driver_selection_tests;
mod command_driver_tests;
mod command_evidence_tests;
mod command_tests;
mod command_work_tests;
mod first_message_tests;
mod fixtures;
mod ledger_tests;
mod linear_input_tests;
mod live_skill_tests;
mod managed_context_tests;
mod observation_tests;
mod preferences_tests;
mod queued_message_tests;
mod retired_prompt_tests;
mod settings_tests;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use layer3_adapter::{ConversationInput, PermissionDecision};

use crate::sandbox::store::{AgentConversationStore, AgentLogStore, AgentStore};
use crate::{RuntimeCapabilities, RuntimeCapability};

use super::*;
use fixtures::*;

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

#[tokio::test]
async fn approval_answer_preserves_the_opaque_locator_and_rolls_back_safely() {
    let store = Arc::new(InMemorySandboxStore::default());
    let waiting = seed_waiting_approval(&store).await;
    let (sender, inputs) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    service
        .answer_approval(
            &owner(),
            &waiting.id,
            APPROVAL_ID.into(),
            AgentApprovalDecision::Allow,
        )
        .await
        .unwrap();
    assert!(matches!(
        inputs.lock().unwrap().as_slice(),
        [ConversationInput::PermissionAnswer { id, locator: Some(locator), decision: PermissionDecision::Allow }]
            if id == APPROVAL_ID && locator.as_str() == APPROVAL_LOCATOR
    ));
    assert_eq!(
        store.get_agent(&waiting.id).await.unwrap().unwrap().status,
        AgentStatus::Working
    );

    let rollback_store = Arc::new(InMemorySandboxStore::default());
    let waiting = seed_waiting_approval(&rollback_store).await;
    let (sender, _) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender).fail_on_resolution(2);
    let service = AgentOperationService::new(rollback_store.clone(), registry);
    assert!(matches!(
        service
            .answer_approval(
                &owner(),
                &waiting.id,
                APPROVAL_ID.into(),
                AgentApprovalDecision::Deny,
            )
            .await,
        Err(AgentOperationError::RuntimeUnavailable(_))
    ));
    let stored = rollback_store
        .get_agent(&waiting.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, AgentStatus::WaitingApproval);
    let pending = rollback_store
        .pending_agent_approvals(&waiting.id)
        .await
        .unwrap();
    assert!(matches!(
        pending.as_slice(),
        [AgentLogEntry { kind: AgentLogKind::ApprovalRequested { approval_id, runtime_locator: Some(locator), .. }, .. }]
            if approval_id == APPROVAL_ID && locator == APPROVAL_LOCATOR
    ));
}

#[tokio::test]
async fn concurrent_approvals_remain_waiting_and_pre_write_failure_restores_only_its_request() {
    for fail_before_write in [false, true] {
        let store = Arc::new(InMemorySandboxStore::default());
        let waiting = seed_waiting_approval(&store).await;
        let recorder = DurableAgentUpdateRecorder::new(store.clone());
        recorder
            .record_update(
                &waiting.id,
                3,
                &mut AgentMessageAssembly::default(),
                layer3_adapter::ConversationUpdate::Asked(layer3_adapter::PermissionRequest {
                    id: "approval-second".into(),
                    locator: Some(layer3_adapter::PermissionLocator::new("route-second")),
                    request: layer3_adapter::RequestPermissionRequest::new(
                        "conversation",
                        layer3_adapter::ToolCallUpdate::new(
                            "call-second",
                            layer3_adapter::ToolCallUpdateFields::new().title("Read another file"),
                        ),
                        vec![],
                    ),
                }),
                LATER.into(),
            )
            .await
            .unwrap();
        let (sender, inputs) = recording_sender(Ok(()));
        let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
        let registry = if fail_before_write {
            registry.fail_on_resolution(2)
        } else {
            registry
        };
        let service = AgentOperationService::new(store.clone(), registry);
        let result = service
            .answer_approval(
                &owner(),
                &waiting.id,
                APPROVAL_ID.into(),
                AgentApprovalDecision::Allow,
            )
            .await;
        let stored = store.get_agent(&waiting.id).await.unwrap().unwrap();
        assert_eq!(stored.status, AgentStatus::WaitingApproval);
        assert_eq!(
            stored.conversation.active_turn,
            waiting.conversation.active_turn
        );
        let pending = store.pending_agent_approvals(&waiting.id).await.unwrap();
        if fail_before_write {
            assert!(matches!(
                result,
                Err(AgentOperationError::RuntimeUnavailable(_))
            ));
            assert!(inputs.lock().unwrap().is_empty());
            assert_eq!(pending.len(), 2);
        } else {
            result.unwrap();
            assert_eq!(pending.len(), 1);
            assert!(
                matches!(&pending[0].kind, AgentLogKind::ApprovalRequested { approval_id, .. } if approval_id == "approval-second")
            );
            service
                .answer_approval(
                    &owner(),
                    &waiting.id,
                    "approval-second".into(),
                    AgentApprovalDecision::Allow,
                )
                .await
                .unwrap();
            assert_eq!(
                store.get_agent(&waiting.id).await.unwrap().unwrap().status,
                AgentStatus::Working
            );
            assert!(store
                .pending_agent_approvals(&waiting.id)
                .await
                .unwrap()
                .is_empty());
            assert_eq!(inputs.lock().unwrap().len(), 2);
        }
    }
}

#[tokio::test]
async fn approval_answer_requires_the_exact_pending_identity() {
    let store = Arc::new(InMemorySandboxStore::default());
    let waiting = seed_waiting_approval(&store).await;
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(store, registry.clone());

    assert_eq!(
        service
            .answer_approval(
                &owner(),
                &waiting.id,
                "approval-other".into(),
                AgentApprovalDecision::Abort,
            )
            .await,
        Err(AgentOperationError::ApprovalNotPending {
            approval_id: "approval-other".into()
        })
    );
    assert_eq!(registry.resolutions(), 0);
    assert!(inputs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn approval_answer_accepts_only_an_active_waiting_agent() {
    for status in ALL_AGENT_STATUSES {
        if status == AgentStatus::WaitingApproval {
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
                .answer_approval(
                    &owner(),
                    &agent.id,
                    APPROVAL_ID.into(),
                    AgentApprovalDecision::Allow,
                )
                .await,
            Err(AgentOperationError::StatusConflict { status })
        );
        assert_eq!(registry.resolutions(), 0);
        assert!(inputs.lock().unwrap().is_empty());
    }

    let store = Arc::new(InMemorySandboxStore::default());
    let mut waiting = test_agent(AgentStatus::WaitingApproval, None);
    waiting.conversation.active_turn = None;
    store.create_agent(waiting.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(store, registry.clone());
    assert_eq!(
        service
            .answer_approval(
                &owner(),
                &waiting.id,
                APPROVAL_ID.into(),
                AgentApprovalDecision::Allow,
            )
            .await,
        Err(AgentOperationError::NoActiveTurn)
    );
    assert_eq!(registry.resolutions(), 0);
    assert!(inputs.lock().unwrap().is_empty());
}

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
