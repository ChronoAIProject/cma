use super::*;

#[tokio::test]
async fn legacy_retired_prompt_recovery_projects_failure_once_and_preserves_admission() {
    use super::command_work_tests::{admit, fixture, request};
    use crate::sandbox::agent_io::*;
    use crate::sandbox::store::AgentConversationMutation;
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let admitted = admit(
        &store,
        &agent,
        &parent,
        "retired",
        AgentCommand::message("original input"),
    )
    .await;
    let work = store
        .claim_next_command(request(&agent, "worker", &crate::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    let (sender, inputs) = recording_sender(Err(layer3_adapter::AdapterError::Unavailable(
        "conversation link has retired".into(),
    )));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    assert!(matches!(
        service.execute_claimed_command(&owner(), &work).await,
        Err(AgentOperationError::DeliveryAmbiguous { .. })
    ));
    let id = &admitted.receipt.envelope.command_id;
    let before = store.reconcile_command(id).await.unwrap();
    let current = store.get_agent(&agent.id).await.unwrap().unwrap();
    let retained = store.agent_log(&agent.id, 0, 100).await.unwrap();
    let at = crate::now_rfc3339();
    let mut next = current.clone();
    let operation = next.conversation.operation.as_mut().unwrap();
    operation
        .recover_retired_prompt_before_write(at.clone())
        .unwrap();
    let logs = vec![
        AgentLogKind::OperationDeliveryChanged {
            operation_sequence: operation.sequence,
            operation: operation.intent.kind(),
            state: operation.state,
        },
        AgentLogKind::TurnFailed {
            turn_sequence: operation.turn_sequence,
            failure: TypedFailure {
                class: "input_not_written".into(),
                detail: "the retired link refused the original prompt before writing it".into(),
            },
        },
        AgentLogKind::StatusChanged {
            from: AgentStatus::Working,
            to: AgentStatus::Ready,
        },
    ];
    next.conversation.active_turn = None;
    next.transition(AgentStatus::Ready, at.clone(), None)
        .unwrap();
    next.conversation.last_seq += logs.len() as u64;
    store
        .commit_agent_conversation(AgentConversationMutation {
            settings_fence: None,
            command_claim: None,
            execution_fence: None,
            trigger_execution_fence: None,
            admission_guard: None,
            reconciliation_fence: None,
            expected_agent_revision: current.revision,
            expected_last_seq: current.conversation.last_seq,
            runtime_generation: current.conversation.runtime_generation.unwrap(),
            agent: next,
            appended_at: at,
            agent_logs: logs,
        })
        .await
        .unwrap();
    let recovered = store.reconcile_command(id).await.unwrap();
    assert_eq!(recovered.status, AgentResponseStatus::Failed);
    assert_eq!(recovered.receipt, admitted.receipt);
    assert_eq!(recovered.work.dispatch, before.work.dispatch);
    assert_eq!(store.reconcile_command(id).await.unwrap(), recovered);
    assert_eq!(
        &store.agent_log(&agent.id, 0, 100).await.unwrap()[..retained.len()],
        retained
    );
    assert!(store
        .claim_next_command(request(&agent, "next", &crate::now_rfc3339()))
        .await
        .unwrap()
        .is_none());
    assert_eq!(inputs.lock().unwrap().len(), 1);
}

#[test]
fn legacy_retired_prompt_recovery_requires_exact_non_delivery_evidence() {
    let mut original = operation_record(AgentOperationIntent::send_message("retained"));
    original
        .require_recovery(
            AgentOperationFailure::new(
                "input_write_ambiguous",
                "runtime unavailable: conversation link has retired",
                LATER,
            )
            .unwrap(),
        )
        .unwrap();
    for mutation in 0..5 {
        let mut refused = original.clone();
        match mutation {
            0 => refused.write_confirmed_at = Some(LATER.into()),
            1 => {
                refused.failure.as_mut().unwrap().detail =
                    "runtime unavailable: conversation write failed".into()
            }
            2 => refused.intent = AgentOperationIntent::steer("retained"),
            3 => refused.failure.as_mut().unwrap().code = "other".into(),
            _ => refused.state = AgentOperationDelivery::WriteConfirmed,
        }
        let before = refused.clone();
        assert!(refused
            .recover_retired_prompt_before_write(LATEST.into())
            .is_err());
        assert_eq!(refused, before);
    }
    let mut recovered = original.clone();
    recovered
        .recover_retired_prompt_before_write(LATEST.into())
        .unwrap();
    assert_eq!(recovered.state, AgentOperationDelivery::PreWriteFailed);
    assert!(!recovered.blocks_dispatch());
    assert_eq!(recovered.intent, original.intent);
    assert_eq!(recovered.sequence, original.sequence);
    assert_eq!(recovered.turn_sequence, original.turn_sequence);
    assert_eq!(recovered.dispatch_fenced_at, original.dispatch_fenced_at);
    assert_eq!(recovered.runtime_generation, original.runtime_generation);
}

#[tokio::test]
async fn retired_prompt_before_write_preserves_message_without_stranding_active_turn() {
    let store = Arc::new(InMemorySandboxStore::default());
    let ready = test_agent(AgentStatus::Ready, None);
    seed_active_parent(&store).await;
    store.create_agent(ready.clone()).await.unwrap();
    let (sender, _) = recording_sender(Err(layer3_adapter::AdapterError::PromptNotWritten(
        "conversation link has retired".into(),
    )));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(store.clone(), registry);
    assert!(matches!(
        service
            .send_message(&owner(), &ready.id, "Retain this original message.".into())
            .await,
        Err(AgentOperationError::RuntimeUnavailable(_))
    ));
    let stored = store.get_agent(&ready.id).await.unwrap().unwrap();
    assert_eq!(stored.status, AgentStatus::Ready);
    assert_eq!(stored.conversation.active_turn, None);
    assert_eq!(
        stored.conversation.conversation_id,
        ready.conversation.conversation_id
    );
    assert_eq!(
        stored.conversation.runtime_generation,
        ready.conversation.runtime_generation
    );
    assert_eq!(
        stored.conversation.operation.unwrap().state,
        AgentOperationDelivery::PreWriteFailed
    );
    let log = store.agent_log(&ready.id, 0, 20).await.unwrap();
    assert_eq!(log.iter().filter(|e| matches!(&e.kind, AgentLogKind::OwnerMessage { content } if content == "Retain this original message.")).count(), 1);
    assert_eq!(log.iter().filter(|e| matches!(&e.kind, AgentLogKind::TurnFailed { failure, .. } if failure.class == "input_not_written")).count(), 1);
}
