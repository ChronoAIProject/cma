use super::*;

#[tokio::test]
async fn terminal_only_recovery_preserves_deleted_rows_and_delivery_history() {
    let (store, id) = fixture();
    {
        let mut state = store.lock();
        let agent = state.agents.values_mut().next().unwrap();
        agent.status = AgentStatus::Deleted;
        agent.deleted_at = Some(NOW.into());
        agent.work.receipts.clear();
        let mut first =
            crate::sandbox::FirstMessageRecord::new("original-request", "retained input", NOW)
                .unwrap();
        first.fence_dispatch_for(8, 9, 6, NOW.into()).unwrap();
        first.mark_write_confirmed(NOW.into()).unwrap();
        agent.conversation.first_message = Some(first);
        agent.conversation.active_turn = Some(8);
        let mut another = agent.clone();
        another.id = AgentId("agt_deleted_second".into());
        state
            .agent_logs
            .insert(another.id.clone(), vec![log(another.id.as_str(), 1, NOW)]);
        state.agents.insert(another.id.clone(), another);
    }
    let before = store
        .unpublished_runtime_recovery_snapshot(&id)
        .await
        .unwrap();
    let original_logs = store.lock().agent_logs.clone();
    let native_proof = proof(&before);
    assert!(native_proof.conversations.is_empty());
    let admitted = store
        .admit_unpublished_runtime_recovery(SandboxUnpublishedRecoveryRequest {
            proof: native_proof,
            snapshot: before.clone(),
            target_runtime_version: "image-latest".into(),
            released_runtime: None,
            automatic: false,
            requested_at: NOW.into(),
        })
        .await
        .unwrap();
    let Some(crate::sandbox::SandboxLifecycleOperation::ImageRotation { identity, .. }) =
        admitted.lifecycle.operation
    else {
        panic!()
    };
    assert!(identity.agents.is_empty());
    for original in &before.agents {
        assert_eq!(
            store.get_agent(&original.id).await.unwrap().as_ref(),
            Some(original)
        );
        assert_eq!(original.status, AgentStatus::Deleted);
    }
    assert_eq!(store.lock().agent_logs, original_logs);
}

pub(super) struct RefuseUnverifiableProcess;
#[async_trait]
impl SandboxUnpublishedRecoveryVerifier for RefuseUnverifiableProcess {
    async fn verify(
        &self,
        _: &SandboxUnpublishedRecoverySnapshot,
    ) -> Result<SandboxUnpublishedRecoveryProof, SandboxStoreError> {
        Err(SandboxStoreError::InvalidTransaction(
            "native process identity cannot be verified",
        ))
    }
}

#[tokio::test]
async fn public_update_cannot_replace_runtime_when_native_process_proof_is_refused() {
    let (store, id) = fixture();
    let store = Arc::new(store);
    let before = store.get_sandbox(&id).await.unwrap().unwrap();
    let service = SandboxRuntimeUpdateService::new(store.clone(), "image-latest".into())
        .with_unpublished_recovery(Arc::new(RefuseUnverifiableProcess));
    let caller = crate::Caller {
        subject: before.owner.clone(),
        authorization: crate::Authorization::new([crate::Capability::UseOwnSandboxes]),
    };
    assert!(service.request(&caller, &id, "image-latest").await.is_err());
    assert_eq!(store.get_sandbox(&id).await.unwrap().unwrap(), before);
    assert!(!store.lock().sandbox_events.contains_key(&id));
}

pub(super) struct VerifyOriginalProcesses;
#[async_trait]
impl SandboxUnpublishedRecoveryVerifier for VerifyOriginalProcesses {
    async fn verify(
        &self,
        snapshot: &SandboxUnpublishedRecoverySnapshot,
    ) -> Result<SandboxUnpublishedRecoveryProof, SandboxStoreError> {
        Ok(proof(snapshot))
    }
}

#[tokio::test]
async fn explicit_recovery_pins_requested_latest_while_automatic_recovery_keeps_recorded_image() {
    for automatic in [false, true] {
        let (store, id) = fixture();
        let store = Arc::new(store);
        let before = store.get_sandbox(&id).await.unwrap().unwrap();
        let latest = if automatic {
            before.runtime.as_ref().unwrap().runtime_version.clone()
        } else {
            "image-latest".into()
        };
        let service = SandboxRuntimeUpdateService::new(store.clone(), latest)
            .with_unpublished_recovery(Arc::new(VerifyOriginalProcesses));
        let after = if automatic {
            service.recover_failed_unpublished(&id).await.unwrap()
        } else {
            let caller = crate::Caller {
                subject: before.owner.clone(),
                authorization: crate::Authorization::new([crate::Capability::UseOwnSandboxes]),
            };
            service.request(&caller, &id, "image-latest").await.unwrap()
        };
        let expected_target = if automatic {
            before.runtime.as_ref().unwrap().runtime_version.as_str()
        } else {
            "image-latest"
        };
        assert_eq!(
            after.admitted_runtime_update_target(),
            Some(expected_target)
        );
        assert_eq!(after.runtime_update_request().unwrap().automatic, automatic);
        assert_eq!(after.lifecycle.home, before.lifecycle.home);
        assert!(after.require_unpublished_runtime_quiescence().is_err());
    }
}

#[tokio::test]
async fn automatic_recovery_does_not_race_an_explicit_upgrade_for_an_obsolete_target() {
    let (store, id) = fixture();
    let store = Arc::new(store);
    let before = store.get_sandbox(&id).await.unwrap().unwrap();
    let service = SandboxRuntimeUpdateService::new(store.clone(), "new-deployment-image".into())
        .with_unpublished_recovery(Arc::new(VerifyOriginalProcesses));
    assert!(matches!(
        service.recover_failed_unpublished(&id).await,
        Err(crate::sandbox::SandboxRuntimeUpdateError::NotAvailable)
    ));
    assert_eq!(store.get_sandbox(&id).await.unwrap().unwrap(), before);
    assert!(!store.lock().sandbox_events.contains_key(&id));
}

#[tokio::test]
async fn generic_saves_cannot_bypass_recovery_admission_or_quiescence_audit() {
    let (store, id) = fixture();
    let snapshot = store
        .unpublished_runtime_recovery_snapshot(&id)
        .await
        .unwrap();
    let request = SandboxUnpublishedRecoveryRequest {
        proof: proof(&snapshot),
        snapshot: snapshot.clone(),
        target_runtime_version: "image-latest".into(),
        released_runtime: None,
        automatic: false,
        requested_at: NOW.into(),
    };
    let admitted = crate::sandbox::apply_unpublished_runtime_recovery(&snapshot, &request)
        .unwrap()
        .sandbox;
    assert!(matches!(
        store
            .save_sandbox(snapshot.sandbox.revision, admitted)
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    store
        .admit_unpublished_runtime_recovery(request)
        .await
        .unwrap();
    let current = store
        .claim_sandbox_work(
            &id,
            "recovery-worker",
            NOW.into(),
            "2026-09-05T00:45:00Z".into(),
        )
        .await
        .unwrap()
        .unwrap();
    let recovery = current.lifecycle.unpublished_recovery.as_ref().unwrap();
    let proof = &recovery.proof;
    let receipt = crate::sandbox::SandboxUnpublishedRecoveryQuiescence {
        operation_id: recovery.operation_id.clone(),
        runtime_id: proof.runtime_id.clone(),
        runtime_generation: proof.runtime_generation,
        home_identity: proof.home_identity.clone(),
        boot_id: proof.boot_id.clone(),
        provider_plan_sha256: proof.provider_plan_sha256.clone(),
        runtime_process_absent: true,
        conversations: proof.conversations.clone(),
        completed_at: NOW.into(),
    };
    let mut next = current.clone();
    next.complete_unpublished_runtime_quiescence(
        receipt.clone(),
        "recovery-worker",
        current.work.lease.as_ref().unwrap().fence,
        NOW.into(),
    )
    .unwrap();
    let event = SandboxEventKind::RuntimeRecoveryQuiesced {
        receipt: Box::new(receipt),
    };
    let events = store.lock().sandbox_events.clone();
    assert!(matches!(
        store.save_sandbox(current.revision, next.clone()).await,
        Err(SandboxStoreError::Conflict)
    ));
    assert!(matches!(
        store
            .commit(SandboxTransaction::SaveSandboxAndAppend {
                command_revival: None,
                released_runtime: None,
                expected_revision: current.revision,
                sandbox: Box::new(next.clone()),
                event_at: NOW.into(),
                event: event.clone(),
            })
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(store.get_sandbox(&id).await.unwrap().unwrap(), current);
    assert_eq!(store.lock().sandbox_events, events);
    store
        .commit_sandbox_lifecycle(SandboxLifecycleMutation {
            guard: SandboxMutationGuard::capture(&current),
            sandbox: next.clone(),
            agents: vec![],
            agent_scope: SandboxLifecycleAgentScope::Touched,
            appended_at: NOW.into(),
            events: vec![
                event,
                SandboxEventKind::WorkStepCompleted {
                    step: SandboxWorkStep::RunLifecycleOperation,
                },
            ],
        })
        .await
        .unwrap();
    assert_eq!(store.get_sandbox(&id).await.unwrap().unwrap(), next);
    let revision = next.revision;
    next.begin_retirement(NOW.into()).unwrap();
    store.save_sandbox(revision, next).await.unwrap();
}
