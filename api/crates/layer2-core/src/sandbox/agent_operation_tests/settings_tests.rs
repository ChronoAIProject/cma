use super::*;
use crate::profiles::{PermissionLevel, ThinkingEffort};

fn desired() -> AgentSettings {
    AgentSettings {
        model: "gpt-6".into(),
        reasoning_effort: ThinkingEffort::High,
        permission_mode: PermissionLevel::Autonomous,
    }
}

#[tokio::test]
async fn selected_model_is_atomically_retained_with_each_dispatched_turn() {
    let store = Arc::new(InMemorySandboxStore::default());
    let ready = test_agent(AgentStatus::Ready, None);
    seed_active_parent(&store).await;
    store.create_agent(ready.clone()).await.unwrap();
    let (sender, _) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let selection = AgentSettings {
        model: "selected-alternate".into(),
        ..desired()
    };
    service
        .update_settings(&owner(), &ready.id, selection.clone())
        .await
        .unwrap();
    service
        .send_message(&owner(), &ready.id, "use selected model".into())
        .await
        .unwrap();
    let entries = store.agent_log(&ready.id, 0, 30).await.unwrap();
    let models: Vec<_> = entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            AgentLogKind::TurnStarted { model, .. } => Some(model.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(models, [Some(selection.model)]);
    assert_eq!(
        store.get_agent(&ready.id).await.unwrap().unwrap().profile,
        ready.profile
    );
    let serialized = serde_json::to_vec(&entries).unwrap();
    assert_eq!(
        serde_json::from_slice::<Vec<AgentLogEntry>>(&serialized).unwrap(),
        entries
    );
}

#[tokio::test]
async fn settings_fence_before_rpc_preserve_profile_and_replay_without_second_effect() {
    let store = Arc::new(InMemorySandboxStore::default());
    let ready = test_agent(AgentStatus::Ready, None);
    seed_active_parent(&store).await;
    store.create_agent(ready.clone()).await.unwrap();
    let saved = store.clone();
    let id = ready.id.clone();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = calls.clone();
    let sender = layer3_adapter::ConversationInputSender::spawn(2, move |input| {
        let saved = saved.clone();
        let id = id.clone();
        let seen = seen.clone();
        async move {
            let current = saved.get_agent(&id).await.unwrap().unwrap();
            assert!(current.settings_pending());
            assert_eq!(current.settings(), desired());
            assert!(
                matches!(input, ConversationInput::Configure { model, permission:layer3_adapter::PermissionLevel::Autonomous, .. } if model == "gpt-6")
            );
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    });
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let updated = service
        .update_settings(&owner(), &ready.id, desired())
        .await
        .unwrap();
    assert_eq!(updated.profile, ready.profile);
    assert_eq!(updated.status, AgentStatus::Ready);
    assert_eq!(
        updated.conversation.conversation_id,
        ready.conversation.conversation_id
    );
    assert!(!updated.settings_pending());
    assert_eq!(
        updated.conversation.operation.as_ref().unwrap().state,
        AgentOperationDelivery::ExecutionObserved
    );
    assert_eq!(
        service
            .update_settings(&owner(), &ready.id, desired())
            .await
            .unwrap(),
        updated
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        store
            .get_agent(&ready.id)
            .await
            .unwrap()
            .unwrap()
            .settings(),
        desired()
    );
}

#[tokio::test]
async fn settings_authorization_busy_queue_and_recovery_refuse_before_runtime_effect() {
    for case in 0..5 {
        let store = Arc::new(InMemorySandboxStore::default());
        let mut ready = test_agent(AgentStatus::Ready, None);
        seed_active_parent(&store).await;
        if case == 2 {
            ready.status = AgentStatus::Working;
            ready.conversation.active_turn = Some(7);
        }
        if case == 3 {
            ready
                .conversation
                .queued_messages
                .push(QueuedAgentMessage::new(1, "queued".into(), NOW.into()).unwrap());
        }
        if case == 4 {
            ready
                .work
                .schedule(AgentWorkStep::PublishConnection, NOW.into());
        }
        store.create_agent(ready.clone()).await.unwrap();
        let (sender, inputs) = recording_sender(Ok(()));
        let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
        let service = AgentOperationService::new(store, registry.clone());
        let caller = match case {
            0 => unauthorized_owner(),
            1 => other_owner(),
            _ => owner(),
        };
        assert!(service
            .update_settings(&caller, &ready.id, desired())
            .await
            .is_err());
        assert_eq!(registry.resolutions(), 0);
        assert!(inputs.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn settings_prewrite_failure_restores_previous_value_and_allows_future_input() {
    let store = Arc::new(InMemorySandboxStore::default());
    let ready = test_agent(AgentStatus::Ready, None);
    seed_active_parent(&store).await;
    store.create_agent(ready.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender).fail_on_resolution(2);
    let service = AgentOperationService::new(store.clone(), registry);
    assert!(matches!(
        service
            .update_settings(&owner(), &ready.id, desired())
            .await,
        Err(AgentOperationError::RuntimeUnavailable(_))
    ));
    let saved = store.get_agent(&ready.id).await.unwrap().unwrap();
    assert_eq!(saved.settings(), ready.settings());
    assert!(!saved.settings_pending());
    assert!(inputs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn uncertain_settings_block_input_until_exact_replayed_settings_are_confirmed() {
    let store = Arc::new(InMemorySandboxStore::default());
    let ready = test_agent(AgentStatus::Ready, None);
    seed_active_parent(&store).await;
    store.create_agent(ready.clone()).await.unwrap();
    let (sender, _) = recording_sender(Err(layer3_adapter::AdapterError::Unavailable(
        "response lost".into(),
    )));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    assert!(matches!(
        service
            .update_settings(&owner(), &ready.id, desired())
            .await,
        Err(AgentOperationError::DeliveryAmbiguous { .. })
    ));
    let mut saved = store.get_agent(&ready.id).await.unwrap().unwrap();
    assert!(saved.settings_pending());
    assert_eq!(saved.settings(), desired());
    assert!(matches!(
        service
            .send_message(&owner(), &ready.id, "next".into())
            .await,
        Err(AgentOperationError::DispatchRecoveryRequired { .. })
    ));
    // A replacement process restores the exact serialized desired settings.
    saved = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
    saved
        .work
        .schedule(AgentWorkStep::PublishConnection, NOW.into());
    saved.work.next_fence = 3;
    saved.work.lease = Some(AgentWorkLease {
        holder: WORKER.into(),
        fence: 2,
        expires_at: "2100-01-01T00:00:00Z".into(),
    });
    saved.revision = 1;
    let recovered_store = Arc::new(InMemorySandboxStore::default());
    recovered_store.create_agent(saved.clone()).await.unwrap();
    for entry in store.agent_log(&ready.id, 0, 20).await.unwrap() {
        recovered_store
            .append_agent_log(&ready.id, entry.appended_at, entry.kind)
            .await
            .unwrap();
    }
    let (sender, _) = recording_sender(Ok(()));
    let recovered = AgentOperationService::new(
        recovered_store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let target = super::super::agent_operation_dispatch::runtime_target(&saved).unwrap();
    let authority = AgentWorkAuthority::new(WORKER.into(), 2).unwrap();
    let mut wrong = desired();
    wrong.model = "other".into();
    assert!(matches!(
        recovered
            .confirm_settings_replayed(&target, &authority, &wrong)
            .await,
        Err(AgentOperationError::StaleWorkAuthority)
    ));
    recovered
        .confirm_settings_replayed(&target, &authority, &desired())
        .await
        .unwrap();
    let saved = recovered_store.get_agent(&saved.id).await.unwrap().unwrap();
    assert!(!saved.settings_pending());
    assert_eq!(saved.settings(), desired());
}

#[tokio::test]
async fn unsupported_settings_roll_back_and_leave_conversation_editable() {
    let store = Arc::new(InMemorySandboxStore::default());
    let ready = test_agent(AgentStatus::Ready, None);
    seed_active_parent(&store).await;
    store.create_agent(ready.clone()).await.unwrap();
    let (sender, _) = recording_sender(Err(layer3_adapter::AdapterError::Unsupported(
        "unsupported model".into(),
    )));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    assert!(matches!(
        service
            .update_settings(&owner(), &ready.id, desired())
            .await,
        Err(AgentOperationError::InvalidRequest(_))
    ));
    let saved = store.get_agent(&ready.id).await.unwrap().unwrap();
    assert_eq!(saved.settings(), ready.settings());
    assert!(!saved.settings_pending());
    assert_eq!(
        saved.conversation.operation.as_ref().unwrap().state,
        AgentOperationDelivery::PreWriteFailed
    );
}
