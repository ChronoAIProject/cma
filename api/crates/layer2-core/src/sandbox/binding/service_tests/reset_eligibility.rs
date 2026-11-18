use super::*;

#[tokio::test]
async fn partition_reset_eligibility_distinguishes_absence_lifecycle_and_cleanup() {
    let f = fixture();
    let binding = f.ready().await;
    let mapping = resource_access::ready_conversation(&f, &binding).await;
    reset::delete(&f, &mapping).await;
    let agent = f.store.get_agent(&mapping.agent_id).await.unwrap().unwrap();
    let sandbox = f
        .store
        .get_sandbox(&binding.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let workspace = f
        .store
        .get_workspace(&binding.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let check = |binding: &ProviderBinding, agent: Option<&Agent>| {
        reset_eligibility(binding, &mapping, agent, Some(&sandbox), Some(&workspace))
    };
    assert!(check(&binding, Some(&agent)).is_ok());
    assert!(matches!(
        check(&binding, None),
        Err(BindingError::ResetRefused("partition_reset_agent_missing"))
    ));
    for status in [
        AgentStatus::Ready,
        AgentStatus::Working,
        AgentStatus::Stopped,
        AgentStatus::Failed,
        AgentStatus::Paused,
    ] {
        let mut value = agent.clone();
        value.status = status;
        assert!(matches!(
            check(&binding, Some(&value)),
            Err(BindingError::ResetRefused(
                "partition_reset_agent_not_deleted"
            ))
        ));
    }
    let mut value = agent.clone();
    value.work.step = crate::sandbox::AgentWorkStep::EnsureProcessClosed;
    assert!(matches!(
        check(&binding, Some(&value)),
        Err(BindingError::ResetRefused(
            "partition_reset_cleanup_pending"
        ))
    ));
    let mut value = binding.clone();
    value.profile.session_affinity = SessionAffinity::SourceUser;
    assert!(matches!(
        check(&value, Some(&agent)),
        Err(BindingError::ResetRefused(
            "partition_reset_affinity_unsupported"
        ))
    ));
    let mut value = binding.clone();
    value.existing_agent = Some(ExistingBindingAgent {
        agent_id: agent.id.clone(),
        worktree_id: agent.worktree_id.clone(),
    });
    assert!(matches!(
        check(&value, Some(&agent)),
        Err(BindingError::ResetRefused("partition_reset_existing_agent"))
    ));
    let mut value = binding.clone();
    value.state = BindingState::Retired;
    assert!(matches!(
        check(&value, Some(&agent)),
        Err(BindingError::ResetRefused(
            "partition_reset_binding_not_ready"
        ))
    ));
    assert!(matches!(
        reset_eligibility(&binding, &mapping, Some(&agent), None, Some(&workspace)),
        Err(BindingError::ResetRefused(
            "partition_reset_parents_unavailable"
        ))
    ));
}

#[tokio::test]
async fn binding_service_requires_both_capabilities_and_owner_isolation() {
    let f = fixture();
    let value = profile("Original instruction");
    let digest = format!("sha256:{}", content_digest(&value).unwrap());
    for capability in [
        Capability::ManageGatewayRecords,
        Capability::ManageProviderBindings,
    ] {
        let caller = Caller::new(f.caller.subject.clone(), Authorization::new([capability]));
        assert!(matches!(
            f.service
                .put(&caller, "binding-one", value.clone(), &digest, &f.key)
                .await,
            Err(BindingError::Forbidden)
        ));
    }
    f.put("binding-one", "Original instruction").await;
    let other = Caller::new(
        SubjectId::new("another-manager"),
        f.caller.authorization.clone(),
    );
    assert!(matches!(
        f.service.read(&other, "binding-one").await,
        Err(BindingError::NotFound)
    ));
    f.service.retire(&f.caller, "binding-one").await.unwrap();
    for _ in 0..3 {
        let value = f.binding().await;
        f.service.reconcile_binding(&value).await.unwrap();
    }
    assert!(f.custody.keys.lock().unwrap().is_empty());
    assert_eq!(f.binding().await.work, BindingWork::Idle);
    assert!(matches!(
        f.service
            .put(&f.caller, "binding-one", value, &digest, &f.key)
            .await,
        Err(BindingError::Conflict)
    ));
}

#[tokio::test]
async fn reset_promotes_consumer_and_receipt_scope_without_changing_healthy_heads() {
    let f = fixture();
    let binding = f.ready().await;
    let mapping = resource_access::ready_conversation(&f, &binding).await;
    reset::delete(&f, &mapping).await;
    let scope = ProviderBinding::new(
        "receipt-scope".into(),
        binding.manager.clone(),
        binding.profile.clone(),
        "b".repeat(64),
        crate::now_rfc3339(),
    )
    .unwrap();
    f.store.save_binding(None, scope.clone()).await.unwrap();
    let request = ConversationResetRequest {
        binding_id: binding.binding_id.clone(),
        idempotency_binding_id: scope.binding_id.clone(),
        conversation_id: "scoped-conversation".into(),
        expected_agent_id: mapping.agent_id.clone(),
        owner: binding.resource_owner(),
        manager: binding.manager.clone(),
        operation_id: content_digest(&"scope-reset").unwrap(),
        request_digest: content_digest(&"scope-body").unwrap(),
    };
    let receipt = f
        .service
        .reset_conversation(&f.caller, request.clone(), true)
        .await
        .unwrap();
    for previous in [&binding, &scope] {
        let current = f
            .store
            .binding(&previous.binding_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.revision, previous.revision + 1);
        assert_eq!(current.reset_format, Some(ConversationResetFormat::V1));
        let mut stale = previous.clone();
        stale.advance_revision(crate::now_rfc3339()).unwrap();
        assert!(matches!(
            f.store.save_binding(Some(previous.revision), stale).await,
            Err(crate::sandbox::store::SandboxStoreError::Conflict)
        ));
    }
    let heads = f
        .store
        .conversation_reset_state(&binding.binding_id)
        .await
        .unwrap();
    let receipts = f
        .store
        .conversation_reset_state(&scope.binding_id)
        .await
        .unwrap();
    assert_eq!(heads.heads.len(), 1);
    assert!(heads.receipts.is_empty());
    assert_eq!(receipts.receipts.len(), 1);
    assert!(receipts.heads.is_empty());
    assert_eq!(
        f.service
            .reset_conversation(&f.caller, request.clone(), false)
            .await
            .unwrap(),
        receipt
    );
    let mut changed_consumer = request;
    changed_consumer.binding_id = scope.binding_id;
    assert!(matches!(
        f.service
            .reset_conversation(&f.caller, changed_consumer, true)
            .await,
        Err(BindingError::ResetRefused("partition_reset_key_conflict"))
    ));
}

#[tokio::test]
async fn reset_preserves_exact_predecessor_replay_and_continuation_lifecycle() {
    use super::command_tests::{request, LocalAuthority};
    use crate::{resource_access::ResourceAccessService, sandbox::agent_io::*};
    let f = fixture();
    let binding = f.ready().await;
    let mapping = resource_access::ready_conversation(&f, &binding).await;
    let bindings = BindingAccessService::new(f.store.clone());
    let authority = Arc::new(LocalAuthority(bindings.clone()));
    let access =
        ResourceAccessService::new(f.store.clone(), authority).with_bindings(bindings.clone());
    let mut original = request(&f, &bindings, &access, "original-before-reset").await;
    super::event_tests::attach(&mut original);
    let CommandReservationResult::Created(parent) =
        f.store.reserve_command(original.clone()).await.unwrap()
    else {
        panic!("new parent")
    };
    let mut continuation = request(&f, &bindings, &access, "transcript-before-reset").await;
    continuation.envelope.continuation = Some(ExternalEventContinuation {
        parent_command_id: parent.receipt.envelope.command_id.clone(),
        parent_response_id: parent.receipt.response_id.clone().unwrap(),
        parent_fingerprint: parent.receipt.envelope.request_fingerprint.clone(),
        binding_generation: binding.generation,
        source_generation: None,
        source_expires_at: (crate::seconds_from_rfc3339(&continuation.queue_deadline).unwrap()
            + 3600) as i64,
        artifact_id: "transcript-before-reset".into(),
        source_proof: json!({"proof":"synthetic"}),
    });
    let CommandReservationResult::Created(accepted) =
        f.store.reserve_command(continuation.clone()).await.unwrap()
    else {
        panic!("new continuation")
    };
    reset::delete(&f, &mapping).await;
    let receipt = f
        .service
        .reset_conversation(
            &f.caller,
            ConversationResetRequest {
                binding_id: binding.binding_id.clone(),
                idempotency_binding_id: binding.binding_id.clone(),
                conversation_id: mapping.conversation_id.clone(),
                expected_agent_id: mapping.agent_id.clone(),
                owner: binding.resource_owner(),
                manager: binding.manager.clone(),
                operation_id: content_digest(&"continuation-reset").unwrap(),
                request_digest: content_digest(&"reset-body").unwrap(),
            },
            true,
        )
        .await
        .unwrap();
    let current = f.binding().await;
    let agent = f.store.get_agent(&mapping.agent_id).await.unwrap().unwrap();
    for (input, retained) in [(&mut original, &parent), (&mut continuation, &accepted)] {
        input.binding.as_mut().unwrap().binding = current.clone();
        input.expected_agent_revision = agent.revision;
        assert_eq!(
            f.store.reserve_command(input.clone()).await.unwrap(),
            CommandReservationResult::Existing(retained.clone())
        );
        assert_eq!(retained.receipt.envelope.agent_id, mapping.agent_id);
    }
    let mut late = continuation;
    late.envelope.command_id = crate::ids::AgentCommandId::generate();
    late.response_id = Some(crate::ids::AgentResponseId::generate());
    let proof = late.envelope.continuation.as_mut().unwrap();
    proof.artifact_id = "transcript-after-reset".into();
    late.key_sha256 = command_key_digest(&proof.key()).unwrap();
    assert_eq!(
        f.store.reserve_command(late).await,
        Err(CommandLedgerError::NotAdmissible)
    );
    assert_eq!(
        f.store
            .get_command(&accepted.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(*accepted)
    );
    assert!(f
        .store
        .get_agent(&receipt.head.successor.agent_id)
        .await
        .unwrap()
        .is_none());
}
