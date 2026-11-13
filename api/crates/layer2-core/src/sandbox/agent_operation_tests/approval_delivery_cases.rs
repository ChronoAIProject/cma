use super::*;

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
