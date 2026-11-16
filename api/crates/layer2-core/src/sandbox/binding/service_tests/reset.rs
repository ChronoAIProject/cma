use super::{
    command_tests::{request, LocalAuthority},
    resource_access::{context, ready_conversation},
    *,
};
use crate::{resource_access::ResourceAccessService, sandbox::agent_io::*};

pub(super) async fn delete(f: &Fixture, mapping: &BindingConversation) {
    let mut agent = f.store.get_agent(&mapping.agent_id).await.unwrap().unwrap();
    let revision = agent.revision;
    agent
        .transition(
            AgentStatus::Stopped,
            crate::now_rfc3339(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    f.store.save_agent(revision, agent.clone()).await.unwrap();
    let revision = agent.revision;
    agent
        .transition(AgentStatus::Deleted, crate::now_rfc3339(), None)
        .unwrap();
    agent.work = crate::sandbox::AgentWorkState::new(
        crate::sandbox::AgentWorkStep::None,
        crate::now_rfc3339(),
    );
    f.store.save_agent(revision, agent).await.unwrap();
}
fn reset_request(
    binding: &ProviderBinding,
    mapping: &BindingConversation,
    key: &str,
) -> ConversationResetRequest {
    ConversationResetRequest {
        binding_id: binding.binding_id.clone(),
        idempotency_binding_id: binding.binding_id.clone(),
        conversation_id: "scoped-conversation".into(),
        expected_agent_id: mapping.agent_id.clone(),
        owner: binding.resource_owner(),
        manager: binding.manager.clone(),
        operation_id: content_digest(&key).unwrap(),
        request_digest: content_digest(&(key, mapping.agent_id.as_str())).unwrap(),
    }
}
async fn ready_successor(f: &Fixture, mapping: &BindingConversation) -> BindingConversation {
    let binding = f.binding().await;
    f.service
        .reconcile_conversation(&binding, mapping)
        .await
        .unwrap();
    let mut ready = mapping.clone();
    ready.state = BindingConversationState::Ready;
    ready.revision += 1;
    f.store
        .save_binding_conversation(binding.revision, Some(mapping.revision), ready.clone())
        .await
        .unwrap();
    ready
}

#[tokio::test]
async fn reset_is_lazy_idempotent_and_preserves_healthy_partition_and_history() {
    for kind in ["human", "organization"] {
        let f = fixture();
        let mut profile = profile("reset");
        profile["metadata"]["owner"]["kind"] = kind.into();
        let binding = f.ready_with_profile(profile, None).await;
        let old = ready_conversation(&f, &binding).await;
        let (healthy, _) = f
            .service
            .ensure_conversation(&f.caller, &binding.binding_id, "healthy-partition")
            .await
            .unwrap();
        delete(&f, &old).await;
        let reset = reset_request(&binding, &old, "one");
        assert!(matches!(
            f.service
                .reset_conversation(&f.caller, reset.clone(), false)
                .await,
            Err(BindingError::ResetRefused("partition_reset_disabled"))
        ));
        let (a, b) = tokio::join!(
            f.service.reset_conversation(&f.caller, reset.clone(), true),
            f.service.reset_conversation(&f.caller, reset.clone(), true)
        );
        let receipt = a.unwrap();
        assert_eq!(receipt, b.unwrap());
        assert_eq!(receipt.head.incarnation, 1);
        assert_eq!(
            f.service
                .reset_conversation(&f.caller, reset.clone(), false)
                .await
                .unwrap(),
            receipt
        );
        assert!(f
            .store
            .get_agent(&receipt.head.successor.agent_id)
            .await
            .unwrap()
            .is_none());
        f.service.reconcile(100).await.unwrap();
        assert!(f
            .store
            .get_agent(&receipt.head.successor.agent_id)
            .await
            .unwrap()
            .is_none());
        let inspected = f
            .service
            .inspect_conversation(&f.caller, &binding.binding_id, "scoped-conversation")
            .await
            .unwrap();
        assert_eq!(inspected.incarnation, 1);
        assert_eq!(inspected.conversation, receipt.head.successor);
        assert_eq!(inspected.transport_conversation_id, "scoped-conversation");
        let (a, b) = tokio::join!(
            f.service
                .ensure_conversation(&f.caller, &binding.binding_id, "scoped-conversation"),
            f.service
                .ensure_conversation(&f.caller, &binding.binding_id, "scoped-conversation")
        );
        let (a, ac) = a.unwrap();
        let (b, bc) = b.unwrap();
        assert_eq!(a, b);
        assert_ne!(ac, bc);
        let successor = ready_successor(&f, &a).await;
        assert_ne!(successor.agent_id, old.agent_id);
        assert_eq!(
            f.service
                .ensure_conversation(&f.caller, &binding.binding_id, "healthy-partition")
                .await
                .unwrap()
                .0,
            healthy
        );
        assert_eq!(
            f.store
                .binding_conversation(
                    &binding.binding_id,
                    binding.generation,
                    &old.conversation_id
                )
                .await
                .unwrap(),
            Some(old.clone())
        );
        let access = BindingAccessService::new(f.store.clone());
        let principal = access.authenticate(&f.key.sha256()).await.unwrap().unwrap();
        assert_eq!(
            access
                .resolve(&principal, "scoped-conversation")
                .await
                .unwrap()
                .conversation,
            successor
        );
        assert_eq!(
            access
                .resolve(&principal, old.agent_id.as_str())
                .await
                .unwrap()
                .conversation,
            old
        );
        let mut changed = reset.clone();
        changed.request_digest = "f".repeat(64);
        assert!(matches!(
            f.service.reset_conversation(&f.caller, changed, true).await,
            Err(BindingError::ResetRefused("partition_reset_key_conflict"))
        ));
        let stale = reset_request(&binding, &old, "another");
        assert!(matches!(
            f.service.reset_conversation(&f.caller, stale, true).await,
            Err(BindingError::ResetRefused(
                "partition_reset_mapping_changed"
            ))
        ));
        delete(&f, &successor).await;
        let second = f
            .service
            .reset_conversation(&f.caller, reset_request(&binding, &successor, "two"), true)
            .await
            .unwrap();
        assert_eq!(second.head.incarnation, 2);
        assert_ne!(second.head.successor.agent_id, successor.agent_id);
        assert_eq!(
            f.service
                .reset_conversation(&f.caller, reset, true)
                .await
                .unwrap(),
            receipt
        );
    }
}

#[tokio::test]
async fn reset_fences_racing_admission_and_external_event_identity_survives_agents() {
    let f = fixture();
    let binding = f.ready().await;
    let old = ready_conversation(&f, &binding).await;
    let bindings = BindingAccessService::new(f.store.clone());
    let authority = Arc::new(LocalAuthority(bindings.clone()));
    let access = Arc::new(
        ResourceAccessService::new(f.store.clone(), authority.clone())
            .with_bindings(bindings.clone()),
    );
    let commands = AgentCommandService::new(f.store.clone(), access.clone(), authority);
    let mut admitted = request(&f, &bindings, &access, "pre-reset").await;
    super::event_tests::attach(&mut admitted);
    let result = f.store.reserve_command(admitted.clone()).await.unwrap();
    assert!(matches!(result, CommandReservationResult::Created(_)));
    let mut racing = request(&f, &bindings, &access, "racing").await;
    super::event_tests::attach(&mut racing);
    racing
        .envelope
        .command
        .external_event_mut()
        .unwrap()
        .event_id = "racing".into();
    racing.envelope.request_fingerprint = racing.envelope.command.fingerprint().unwrap();
    delete(&f, &old).await;
    let reset = reset_request(&binding, &old, "reset");
    let (reset, race) = tokio::join!(
        f.service.reset_conversation(&f.caller, reset, true),
        f.store.reserve_command(racing.clone())
    );
    let receipt = reset.unwrap();
    assert!(matches!(
        race,
        Err(CommandLedgerError::Conflict | CommandLedgerError::NotAdmissible)
    ));
    assert_eq!(
        f.store.reserve_command(racing).await,
        Err(CommandLedgerError::Conflict)
    );
    let (materialized, _) = f
        .service
        .ensure_conversation(&f.caller, &binding.binding_id, "scoped-conversation")
        .await
        .unwrap();
    let successor = ready_successor(&f, &materialized).await;
    assert_eq!(successor.agent_id, receipt.head.successor.agent_id);
    let mut replay = request(&f, &bindings, &access, "pre-reset").await;
    super::event_tests::attach(&mut replay);
    replay
        .envelope
        .command
        .external_event_mut()
        .unwrap()
        .provider_conversation_id = Some(successor.conversation_id.clone());
    replay.envelope.request_fingerprint = replay.envelope.command.fingerprint().unwrap();
    assert_eq!(
        admitted.envelope.external_event_key(),
        replay.envelope.external_event_key()
    );
    assert_eq!(
        f.store.reserve_command(replay.clone()).await,
        Err(CommandLedgerError::IdempotencyConflict)
    );
    let principal = bindings
        .authenticate(&f.key.sha256())
        .await
        .unwrap()
        .unwrap();
    let retained = commands
        .external_event_context_for_source(
            &context(&principal),
            None,
            &binding.binding_id,
            "scoped-conversation",
            "event",
        )
        .await
        .unwrap();
    assert_eq!(
        &retained,
        admitted.envelope.command.external_event().unwrap()
    );
    // Gateway-queued input has no prior CMA command. Its first admission selects the successor.
    replay
        .envelope
        .command
        .external_event_mut()
        .unwrap()
        .event_id = "queued-before-reset".into();
    replay.envelope.request_fingerprint = replay.envelope.command.fingerprint().unwrap();
    replay.key_sha256 = command_key_digest("queued-before-reset").unwrap();
    let (a, b) = tokio::join!(
        f.store.reserve_command(replay.clone()),
        f.store.reserve_command(replay)
    );
    assert!(matches!(a.unwrap(), CommandReservationResult::Created(_)));
    assert!(matches!(b.unwrap(), CommandReservationResult::Existing(_)));
    let old_command = f
        .store
        .get_external_event_command(&admitted.envelope.external_event_key().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old_command.receipt.envelope.agent_id, old.agent_id);
}

#[tokio::test]
async fn reset_requires_exact_owner_manager_and_terminal_settled_agent() {
    let f = fixture();
    let binding = f.ready().await;
    let old = ready_conversation(&f, &binding).await;
    let reset = reset_request(&binding, &old, "one");
    assert!(matches!(
        f.service
            .reset_conversation(&f.caller, reset.clone(), true)
            .await,
        Err(BindingError::ResetRefused(
            "partition_reset_agent_not_deleted"
        ))
    ));
    let mut foreign = reset.clone();
    foreign.owner = crate::resource_access::ResourceOwner::Organization {
        subject: binding.owner.clone(),
    };
    assert!(matches!(
        f.service.reset_conversation(&f.caller, foreign, true).await,
        Err(BindingError::NotFound)
    ));
    let stranger = Caller::new(SubjectId::new("stranger"), f.caller.authorization.clone());
    assert!(matches!(
        f.service
            .reset_conversation(&stranger, reset.clone(), true)
            .await,
        Err(BindingError::NotFound)
    ));
    let forbidden = Caller::new(f.caller.subject.clone(), Authorization::none());
    assert!(matches!(
        f.service
            .reset_conversation(&forbidden, reset.clone(), true)
            .await,
        Err(BindingError::Forbidden)
    ));
    delete(&f, &old).await;
    let mut sandbox = f
        .store
        .get_sandbox(&binding.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let revision = sandbox.revision;
    sandbox.status = SandboxStatus::Failed;
    sandbox.revision += 1;
    f.store.save_sandbox(revision, sandbox).await.unwrap();
    assert!(matches!(
        f.service.reset_conversation(&f.caller, reset, true).await,
        Err(BindingError::ResetRefused(
            "partition_reset_parents_unavailable"
        ))
    ));
}
