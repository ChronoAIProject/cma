use super::*;
use crate::sandbox::lifecycle_test_support::{agent, log, NOW};
use crate::sandbox::runtime_update_test_support::native;
use crate::sandbox::{
    AgentStatus, AgentWorkReceipt, AgentWorkStep, QueuedAgentMessage,
    SandboxRecoveryConversationProof, SandboxRuntimeUpdateService, SandboxUnpublishedRecoveryProof,
    SandboxUnpublishedRecoveryRequest, SandboxUnpublishedRecoverySnapshot,
    SandboxUnpublishedRecoveryVerifier, SandboxWorkStep, TypedFailure,
};
use std::sync::Arc;

fn fixture() -> (InMemorySandboxStore, SandboxId) {
    let store = InMemorySandboxStore::default();
    let mut sandbox = native();
    sandbox.status = SandboxStatus::Failed;
    sandbox.failure = Some(TypedFailure {
        class: "restore_failed".into(),
        detail: "reopen failed".into(),
    });
    sandbox.lifecycle.runtime_generation_high_water = 2;
    sandbox.runtime.as_mut().unwrap().provision_generation = Some(2);
    sandbox.work.schedule(SandboxWorkStep::None, NOW.into());
    let mut agent = agent("agt_recovery", "native-original", 1);
    agent.status = AgentStatus::Paused;
    agent.conversation.runtime_generation = Some(1);
    agent.work.receipts.push(AgentWorkReceipt {
        step: AgentWorkStep::OpenConversation,
        operation_id: format!("agent:{}:open:v1", agent.id),
        request_sha256: "a".repeat(64),
        completed_at: NOW.into(),
    });
    let id = sandbox.id.clone();
    {
        let mut state = store.lock();
        state.sandboxes.insert(id.clone(), sandbox);
        state
            .agent_logs
            .insert(agent.id.clone(), vec![log(agent.id.as_str(), 1, NOW)]);
        state.agents.insert(agent.id.clone(), agent);
    }
    (store, id)
}

fn proof(snapshot: &SandboxUnpublishedRecoverySnapshot) -> SandboxUnpublishedRecoveryProof {
    SandboxUnpublishedRecoveryProof {
        sandbox_id: snapshot.sandbox.id.clone(),
        runtime_id: snapshot
            .sandbox
            .runtime
            .as_ref()
            .unwrap()
            .runtime_id
            .clone(),
        runtime_generation: snapshot.runtime_generation().unwrap(),
        home_identity: snapshot.home_identity().unwrap(),
        boot_id: snapshot.sandbox.runtime.as_ref().unwrap().boot_id.clone(),
        runtime_process_absent: false,
        requires_native_quiescence: true,
        provider_plan: serde_json::json!({"inspection": "test-process-snapshot"}),
        provider_plan_sha256: SandboxUnpublishedRecoveryProof::provider_plan_digest(
            &serde_json::json!({"inspection": "test-process-snapshot"}),
        )
        .unwrap(),
        conversations: snapshot
            .agents
            .iter()
            .filter(|agent| agent.status.is_live())
            .map(|agent| SandboxRecoveryConversationProof {
                agent_id: agent.id.clone(),
                conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                history_sha256: "b".repeat(64),
            })
            .collect(),
    }
}

#[tokio::test]
async fn recovery_commit_preserves_admitted_queue_logs_and_original_child_rows() {
    let (store, id) = fixture();
    {
        let mut state = store.lock();
        state
            .agents
            .values_mut()
            .next()
            .unwrap()
            .conversation
            .queued_messages
            .push(QueuedAgentMessage::new(2, "must survive recovery".into(), NOW.into()).unwrap());
    }
    let before = store
        .unpublished_runtime_recovery_snapshot(&id)
        .await
        .unwrap();
    let logs = store.lock().agent_logs.clone();
    let request = SandboxUnpublishedRecoveryRequest {
        proof: proof(&before),
        snapshot: before.clone(),
        target_runtime_version: "image-latest".into(),
        automatic: false,
        requested_at: NOW.into(),
    };
    let after = store
        .admit_unpublished_runtime_recovery(request)
        .await
        .unwrap();
    assert_eq!(after.lifecycle.home, before.sandbox.lifecycle.home);
    assert_eq!(after.admitted_runtime_update_target(), Some("image-latest"));
    assert_eq!(
        store.lock().agents.values().cloned().collect::<Vec<_>>(),
        before.agents
    );
    assert_eq!(store.lock().agent_logs, logs);
    assert!(matches!(
        store.lock().sandbox_events[&id][0].kind,
        SandboxEventKind::RuntimeRecoveryAdmitted { .. }
    ));
}

#[tokio::test]
async fn native_verification_race_refuses_new_child_and_preserves_all_state() {
    let (store, id) = fixture();
    let before = store
        .unpublished_runtime_recovery_snapshot(&id)
        .await
        .unwrap();
    let request = SandboxUnpublishedRecoveryRequest {
        proof: proof(&before),
        snapshot: before.clone(),
        target_runtime_version: "image-latest".into(),
        automatic: false,
        requested_at: NOW.into(),
    };
    {
        let mut state = store.lock();
        let mut another = before.agents[0].clone();
        another.id = AgentId("agt_concurrent".into());
        another.conversation.last_seq = 0;
        state.agents.insert(another.id.clone(), another);
    }
    assert!(matches!(
        store.admit_unpublished_runtime_recovery(request).await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(
        store.get_sandbox(&id).await.unwrap().unwrap(),
        before.sandbox
    );
    assert!(!store.lock().sandbox_events.contains_key(&id));
}

#[tokio::test]
async fn prior_publication_in_logs_cannot_be_hidden_by_an_older_agent_projection() {
    for deleted in [false, true] {
        for retired in [false, true] {
            for runtime_generation in [2, 3] {
                let (store, id) = fixture();
                {
                    let mut state = store.lock();
                    let agent = state.agents.values_mut().next().unwrap();
                    agent.work.receipts.clear();
                    agent.conversation.last_seq = 2;
                    if deleted {
                        agent.status = AgentStatus::Deleted;
                        agent.deleted_at = Some(NOW.into());
                    }
                    let agent_id = agent.id.clone();
                    state.agent_logs.get_mut(&agent_id).unwrap().push(log(
                        agent_id.as_str(),
                        2,
                        NOW,
                    ));
                }
                let before = store
                    .unpublished_runtime_recovery_snapshot(&id)
                    .await
                    .unwrap();
                before.validate().unwrap();
                let request = SandboxUnpublishedRecoveryRequest {
                    proof: proof(&before),
                    snapshot: before.clone(),
                    target_runtime_version: "image-latest".into(),
                    automatic: false,
                    requested_at: NOW.into(),
                };
                {
                    let mut state = store.lock();
                    let entry = &mut state.agent_logs.values_mut().next().unwrap()[0];
                    entry.kind = if retired {
                        AgentLogKind::RuntimeGenerationRetired {
                            previous_generation: 1,
                            runtime_generation,
                        }
                    } else {
                        AgentLogKind::ConversationOpened {
                            conversation_id: "native-original".into(),
                            runtime_generation,
                        }
                    };
                }
                assert!(store
                    .unpublished_runtime_recovery_snapshot(&id)
                    .await
                    .is_err());
                assert!(store
                    .admit_unpublished_runtime_recovery(request)
                    .await
                    .is_err());
                assert_eq!(
                    store.get_sandbox(&id).await.unwrap().unwrap(),
                    before.sandbox
                );
                assert!(!store.lock().sandbox_events.contains_key(&id));
            }
        }
    }
}

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

struct RefuseUnverifiableProcess;
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

struct VerifyOriginalProcesses;
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
