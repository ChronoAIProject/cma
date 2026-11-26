use super::*;
struct Ready;
#[async_trait::async_trait]
impl layer3_adapter::ConversationSettingsSupport for Ready {
    async fn system_prompt_supported(&self) -> bool {
        true
    }
    async fn managed_context_receipt(&self, _: u64) -> Option<String> {
        Some("a".repeat(64))
    }
}
const GUIDANCE: &str = "Publish retained documents with the managed document tools.";

#[tokio::test]
async fn transport_refresh_dispatch_and_ack_preserve_the_pinned_guidance() {
    const PINNED: &str = "Guidance captured at admission";
    const OWNER: &str = "  Owner's exact instructions\n";
    struct AdmissionPinned;
    impl AgentInstructionPolicy for AdmissionPinned {
        fn owner_prompt(&self, agent: &Agent) -> String {
            agent.system_prompt().to_owned()
        }
        fn compose(&self, _agent: &Agent, owner: &str) -> String {
            format!("{PINNED}\n\n{owner}")
        }
        fn pinned_managed_guidance(&self, _agent: &Agent) -> Option<&'static str> {
            Some(PINNED)
        }
    }

    let store = Arc::new(InMemorySandboxStore::default());
    let mut ready = test_agent(AgentStatus::Ready, None);
    let prompt = format!("{PINNED}\n\n{OWNER}");
    ready.profile.content.system_prompt = prompt.clone();
    ready.conversation.settings = Some(AgentSettings {
        system_prompt: Some(prompt.clone()),
        ..ready.settings()
    });
    seed_active_parent(&store).await;
    store.create_agent(ready.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(
        full_capabilities(),
        sender.with_settings_support(Arc::new(Ready)),
    );
    let service = AgentOperationService::new(store.clone(), registry)
        .with_instruction_policy(Arc::new(AdmissionPinned));
    assert!(service
        .refresh_managed_context_at_origin(&ready.id, 2, GUIDANCE, &[], Some("https://new.example"))
        .await
        .unwrap());
    assert!(matches!(
        inputs.lock().unwrap().as_slice(),
        [ConversationInput::RefreshContext { guidance, service_origin: Some(origin), .. }]
        if guidance == PINNED && origin == "https://new.example"
    ));
    let saved = store.get_agent(&ready.id).await.unwrap().unwrap();
    assert_eq!(saved.conversation.managed_guidance_revision, 2);
    assert_eq!(saved.system_prompt(), prompt);
    assert_eq!(saved.system_prompt().matches(PINNED).count(), 1);
    assert_eq!(saved.profile, ready.profile);
}

#[tokio::test]
async fn managed_context_ack_is_one_time_and_preserves_owner_state() {
    let store = Arc::new(InMemorySandboxStore::default());
    let mut ready = test_agent(AgentStatus::Ready, None);
    ready.profile.content.system_prompt = "Owner's exact instructions".into();
    ready.conversation.observations.metadata.working_directory =
        Some("/home/cma/retained-work".into());
    seed_active_parent(&store).await;
    store.create_agent(ready.clone()).await.unwrap();
    let saved = store.clone();
    let id = ready.id.clone();
    let sender = layer3_adapter::ConversationInputSender::spawn(1, move |input| {
        let saved = saved.clone();
        let id = id.clone();
        async move {
            let held = saved.get_agent(&id).await.unwrap().unwrap();
            assert_eq!(held.conversation.managed_guidance_revision, 0);
            assert!(held
                .conversation
                .operation
                .as_ref()
                .unwrap()
                .blocks_dispatch());
            assert!(
                matches!(input,ConversationInput::RefreshContext {model,working_directory:Some(cwd),..}
                if model=="model-a" && cwd=="/home/cma/retained-work")
            );
            Ok(())
        }
    });
    let registry = TestRuntimeRegistry::new(
        full_capabilities(),
        sender.with_settings_support(Arc::new(Ready)),
    );
    let service = AgentOperationService::new(store.clone(), registry.clone());
    assert!(service
        .refresh_managed_context(&ready.id, 1, GUIDANCE, &[])
        .await
        .unwrap());
    let updated = store.get_agent(&ready.id).await.unwrap().unwrap();
    assert_eq!(updated.conversation.managed_guidance_revision, 1);
    assert_eq!(updated.profile, ready.profile);
    assert_eq!(updated.settings(), ready.settings());
    assert_eq!(
        updated.conversation.conversation_id,
        ready.conversation.conversation_id
    );
    assert_eq!(
        updated.conversation.observations,
        ready.conversation.observations
    );
    assert_eq!(updated.work, ready.work);
    assert_eq!(updated.sandbox_id, ready.sandbox_id);
    assert_eq!(updated.worktree_id, ready.worktree_id);
    let resolutions = registry.resolutions();
    assert!(!service
        .refresh_managed_context(&ready.id, 1, GUIDANCE, &[])
        .await
        .unwrap());
    assert_eq!(registry.resolutions(), resolutions);
    assert!(store
        .agent_log(&ready.id, 0, 10)
        .await
        .unwrap()
        .iter()
        .all(|entry| matches!(entry.kind, AgentLogKind::OperationDeliveryChanged { .. })));
}

#[tokio::test]
async fn managed_context_defers_active_approval_queue_first_message_and_lifecycle() {
    for case in 0..7 {
        let store = Arc::new(InMemorySandboxStore::default());
        let mut ready = test_agent(
            AgentStatus::Ready,
            if case == 3 { Some("first") } else { None },
        );
        match case {
            0 => {
                ready.status = AgentStatus::Working;
                ready.conversation.active_turn = Some(7);
            }
            1 => {
                ready.status = AgentStatus::WaitingApproval;
                ready.conversation.active_turn = Some(7);
            }
            2 => ready
                .conversation
                .queued_messages
                .push(QueuedAgentMessage::new(1, "queued".into(), NOW.into()).unwrap()),
            4 => ready
                .work
                .schedule(AgentWorkStep::PublishConnection, NOW.into()),
            5 => ready.requested_stop_reason = Some(AgentStopReason::Owner),
            6 => {
                ready.conversation.operation = Some(operation_record(
                    AgentOperationIntent::send_message("uncertain"),
                ))
            }
            _ => {}
        }
        seed_active_parent(&store).await;
        store.create_agent(ready.clone()).await.unwrap();
        let (sender, inputs) = recording_sender(Ok(()));
        let registry = TestRuntimeRegistry::new(
            full_capabilities(),
            sender.with_settings_support(Arc::new(Ready)),
        );
        let service = AgentOperationService::new(store, registry.clone());
        assert!(
            !service
                .refresh_managed_context_at_origin(
                    &ready.id,
                    2,
                    GUIDANCE,
                    &[],
                    Some("https://new.example")
                )
                .await
                .unwrap(),
            "case {case}"
        );
        assert_eq!(registry.resolutions(), 0);
        assert!(inputs.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn managed_context_lost_ack_stays_fenced_until_exact_worker_replay() {
    let store = Arc::new(InMemorySandboxStore::default());
    let mut ready = test_agent(AgentStatus::Ready, None);
    ready.conversation.settings = Some(AgentSettings {
        system_prompt: Some("Old managed\n\nOwner exact text  \n".into()),
        ..ready.settings()
    });
    seed_active_parent(&store).await;
    store.create_agent(ready.clone()).await.unwrap();
    let (sender, _) = recording_sender(Err(layer3_adapter::AdapterError::Unavailable(
        "lost refresh ACK".into(),
    )));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(
            full_capabilities(),
            sender.with_settings_support(Arc::new(Ready)),
        ),
    );
    assert!(matches!(
        service
            .refresh_managed_context_at_origin(
                &ready.id,
                1,
                GUIDANCE,
                &["Old managed".into()],
                Some("https://new.example")
            )
            .await,
        Err(AgentOperationError::DeliveryAmbiguous { .. })
    ));
    let mut saved = store.get_agent(&ready.id).await.unwrap().unwrap();
    assert_eq!(saved.conversation.managed_guidance_revision, 0);
    assert_eq!(saved.settings(), ready.settings());
    assert!(matches!(
        service
            .send_message(&owner(), &ready.id, "next".into())
            .await,
        Err(AgentOperationError::DispatchRecoveryRequired { .. })
    ));
    let (sequence, input) = saved.pending_managed_context().unwrap();
    assert!(
        matches!(input,ConversationInput::RefreshContext{guidance,service_origin:Some(origin),..} if guidance==GUIDANCE && origin=="https://new.example")
    );
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
        TestRuntimeRegistry::new(
            full_capabilities(),
            sender.with_settings_support(Arc::new(Ready)),
        ),
    );
    let target = super::super::agent_operation_dispatch::runtime_target(&saved).unwrap();
    let authority = AgentWorkAuthority::new(WORKER.into(), 2).unwrap();
    let obsolete_store = Arc::new(InMemorySandboxStore::default());
    obsolete_store.create_agent(saved.clone()).await.unwrap();
    for entry in store.agent_log(&ready.id, 0, 20).await.unwrap() {
        obsolete_store
            .append_agent_log(&ready.id, entry.appended_at, entry.kind)
            .await
            .unwrap();
    }
    let (sender, _) = recording_sender(Ok(()));
    let obsolete = AgentOperationService::new(
        obsolete_store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    obsolete
        .retire_managed_context_replayed(&target, &authority, sequence)
        .await
        .unwrap();
    let retired = obsolete_store.get_agent(&ready.id).await.unwrap().unwrap();
    assert_eq!(retired.conversation.managed_guidance_revision, 0);
    assert_eq!(retired.settings(), ready.settings());
    assert_eq!(
        retired.conversation.operation.as_ref().unwrap().state,
        AgentOperationDelivery::RuntimeLost
    );
    assert!(!retired
        .conversation
        .operation
        .as_ref()
        .unwrap()
        .blocks_dispatch());
    assert!(recovered
        .confirm_managed_context_replayed(&target, &authority, sequence + 1)
        .await
        .is_err());
    assert_eq!(
        recovered_store
            .get_agent(&ready.id)
            .await
            .unwrap()
            .unwrap()
            .conversation
            .managed_guidance_revision,
        0
    );
    recovered
        .confirm_managed_context_replayed(&target, &authority, sequence)
        .await
        .unwrap();
    let result = recovered_store.get_agent(&ready.id).await.unwrap().unwrap();
    assert_eq!(result.conversation.managed_guidance_revision, 1);
    assert!(result.pending_managed_context().is_none());
    assert_eq!(
        result.settings(),
        AgentSettings {
            system_prompt: Some(format!("{GUIDANCE}\n\nOwner exact text  \n")),
            ..ready.settings()
        }
    );
    assert_eq!(
        result.conversation.preferences,
        ready.conversation.preferences
    );
    assert_eq!(result.profile, ready.profile);
}

#[tokio::test]
async fn managed_context_owner_command_between_preflight_and_admission_wins() {
    use crate::sandbox::agent_io::{AgentCommandStore, CommandReservation};
    struct Race {
        store: Arc<InMemorySandboxStore>,
        reservation: CommandReservation,
    }
    #[async_trait::async_trait]
    impl layer3_adapter::ConversationSettingsSupport for Race {
        async fn system_prompt_supported(&self) -> bool {
            true
        }
        async fn managed_context_receipt(&self, _: u64) -> Option<String> {
            self.store
                .reserve_command(self.reservation.clone())
                .await
                .unwrap();
            Some("a".repeat(64))
        }
    }
    let store = Arc::new(InMemorySandboxStore::default());
    let ready = test_agent(AgentStatus::Ready, None);
    let parent = seed_active_parent(&store).await;
    store.create_agent(ready.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let sender = sender.with_settings_support(Arc::new(Race {
        store: store.clone(),
        reservation: super::ledger_tests::reservation(&ready, &parent, "during-preflight"),
    }));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    assert!(matches!(
        service
            .refresh_managed_context(&ready.id, 1, GUIDANCE, &[])
            .await,
        Err(AgentOperationError::Conflict)
    ));
    assert!(inputs.lock().unwrap().is_empty());
    assert!(store
        .get_agent(&ready.id)
        .await
        .unwrap()
        .unwrap()
        .conversation
        .operation
        .is_none());
}

#[tokio::test]
async fn managed_context_prewrite_refusal_never_marks_applied_and_can_retry() {
    let store = Arc::new(InMemorySandboxStore::default());
    let ready = test_agent(AgentStatus::Ready, None);
    seed_active_parent(&store).await;
    store.create_agent(ready.clone()).await.unwrap();
    let (sender, _) = recording_sender(Err(layer3_adapter::AdapterError::Unsupported(
        "no context writer".into(),
    )));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(
            full_capabilities(),
            sender.with_settings_support(Arc::new(Ready)),
        ),
    );
    assert!(matches!(
        service
            .refresh_managed_context(&ready.id, 1, GUIDANCE, &[])
            .await,
        Err(AgentOperationError::InvalidRequest(_))
    ));
    let saved = store.get_agent(&ready.id).await.unwrap().unwrap();
    assert_eq!(saved.conversation.managed_guidance_revision, 0);
    assert!(!saved
        .conversation
        .operation
        .as_ref()
        .unwrap()
        .blocks_dispatch());
    let (sender, _) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(
            full_capabilities(),
            sender.with_settings_support(Arc::new(Ready)),
        ),
    );
    assert!(!service
        .refresh_managed_context(&ready.id, 1, GUIDANCE, &[])
        .await
        .unwrap());
    assert!(service
        .refresh_managed_context(&ready.id, 2, GUIDANCE, &[])
        .await
        .unwrap());
}

#[tokio::test]
async fn managed_context_waits_for_the_durable_command_queue() {
    use crate::sandbox::agent_io::AgentCommandStore;
    let store = Arc::new(InMemorySandboxStore::default());
    let ready = test_agent(AgentStatus::Ready, None);
    let parent = seed_active_parent(&store).await;
    store.create_agent(ready.clone()).await.unwrap();
    let reservation = super::ledger_tests::reservation(&ready, &parent, "before-managed-context");
    store.reserve_command(reservation).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(
            full_capabilities(),
            sender.with_settings_support(Arc::new(Ready)),
        ),
    );
    assert!(matches!(
        service
            .refresh_managed_context(&ready.id, 1, GUIDANCE, &[])
            .await,
        Err(AgentOperationError::Conflict)
    ));
    assert!(inputs.lock().unwrap().is_empty());
    assert_eq!(store.get_agent(&ready.id).await.unwrap().unwrap(), ready);
}

#[tokio::test]
async fn managed_context_ack_updates_only_managed_overlay_and_future_settings_keep_it() {
    let store = Arc::new(InMemorySandboxStore::default());
    let mut ready = test_agent(AgentStatus::Ready, None);
    ready.conversation.settings = Some(AgentSettings {
        system_prompt: Some("Old managed\n\nOwner exact text  \n".into()),
        ..ready.settings()
    });
    seed_active_parent(&store).await;
    store.create_agent(ready.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(
            full_capabilities(),
            sender.with_settings_support(Arc::new(Ready)),
        ),
    );
    assert!(service
        .refresh_managed_context(&ready.id, 1, "New managed", &["Old managed".into()])
        .await
        .unwrap());
    let refreshed = store.get_agent(&ready.id).await.unwrap().unwrap();
    assert_eq!(
        refreshed.settings().system_prompt.as_deref(),
        Some("New managed\n\nOwner exact text  \n")
    );
    assert_eq!(
        refreshed.conversation.preferences,
        ready.conversation.preferences
    );
    assert_eq!(refreshed.profile, ready.profile);
    let changed = AgentSettings {
        model: "model-b".into(),
        system_prompt: None,
        ..refreshed.settings()
    };
    service
        .update_settings(&owner(), &ready.id, changed)
        .await
        .unwrap();
    assert!(
        matches!(inputs.lock().unwrap().last(),Some(ConversationInput::Configure {system_prompt:Some(prompt),..}) if prompt=="New managed\n\nOwner exact text  \n")
    );
}
