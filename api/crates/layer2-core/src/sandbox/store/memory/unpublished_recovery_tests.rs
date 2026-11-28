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

#[path = "failed_provision_tests.rs"]
mod failed_provision;

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
        reserved_provision: None,
        observed_provision_generation: None,
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

fn stalled_reopen_fixture() -> (InMemorySandboxStore, SandboxId) {
    use crate::sandbox::{SandboxLifecycleOperation, SandboxReviveCause, SandboxRevivePhase};
    let (store, id) = fixture();
    {
        let mut state = store.lock();
        let sandbox = state.sandboxes.get_mut(&id).unwrap();
        sandbox
            .lifecycle
            .start_revive(&id, NOW.into(), SandboxReviveCause::RuntimeLost)
            .unwrap();
        sandbox.status = SandboxStatus::Reviving;
        sandbox.failure = None;
        let Some(SandboxLifecycleOperation::Revive { phase, .. }) =
            &mut sandbox.lifecycle.operation
        else {
            panic!()
        };
        *phase = SandboxRevivePhase::ConversationReopen;
        sandbox.lifecycle.runtime_generation_high_water = 16;
        sandbox.runtime.as_mut().unwrap().provision_generation = Some(16);
        sandbox.lifecycle.runtime_update = Some(crate::sandbox::SandboxRuntimeUpdateState {
            checked_at: NOW.into(),
            observed_version: "image-queued".into(),
            request: Some(crate::sandbox::SandboxRuntimeUpdateRequest {
                target_version: "image-queued".into(),
                requested_at: NOW.into(),
                automatic: false,
                started_at: None,
                completed_at: None,
                failure: None,
                retained_transition: None,
            }),
        });
        sandbox
            .work
            .schedule(SandboxWorkStep::RunLifecycleOperation, NOW.into());
        sandbox.work.attempts = 4036;
        sandbox.validate().unwrap();
        let agent = state.agents.values_mut().next().unwrap();
        agent.conversation.runtime_generation = Some(15);
        agent.work.receipts.clear();
        agent
            .conversation
            .queued_messages
            .push(QueuedAgentMessage::new(2, "preserved input".into(), NOW.into()).unwrap());
    }
    (store, id)
}

#[tokio::test]
async fn explicit_stalled_reopen_update_supersedes_old_queue_through_owner_and_operator_service() {
    for operator in [false, true] {
        let (store, id) = stalled_reopen_fixture();
        let store = Arc::new(store);
        let before = store
            .unpublished_runtime_recovery_snapshot(&id)
            .await
            .unwrap();
        let logs = store.lock().agent_logs.clone();
        let service = SandboxRuntimeUpdateService::new(store.clone(), "image-latest".into())
            .with_unpublished_recovery(Arc::new(VerifyOriginalProcesses));
        // The public runtime_update projection delegates its can_update field here,
        // even while it reports the older request as queued.
        assert!(service.can_update(&before.sandbox));
        assert!(before
            .sandbox
            .runtime_update_request()
            .unwrap()
            .started_at
            .is_none());
        let after = if operator {
            let caller = crate::Caller {
                subject: crate::SubjectId("operator".into()),
                authorization: crate::Authorization::new([crate::Capability::ControlAnySandbox]),
            };
            service
                .request_as_operator(&caller, &id, "image-latest")
                .await
                .unwrap()
        } else {
            let caller = crate::Caller {
                subject: before.sandbox.owner.clone(),
                authorization: crate::Authorization::new([crate::Capability::UseOwnSandboxes]),
            };
            service.request(&caller, &id, "image-latest").await.unwrap()
        };
        assert_eq!(after.admitted_runtime_update_target(), Some("image-latest"));
        assert_eq!(after.lifecycle.home, before.sandbox.lifecycle.home);
        assert_eq!(
            after.lifecycle.suspended_runtime.as_deref(),
            before.sandbox.runtime.as_ref()
        );
        assert_eq!(
            after.retained_home_recovery().unwrap().source_generation,
            16
        );
        assert_eq!(
            store.lock().agents.values().cloned().collect::<Vec<_>>(),
            before.agents
        );
        assert_eq!(store.lock().agent_logs, logs);
        assert!(after.require_unpublished_runtime_quiescence().is_err());
    }
}

#[tokio::test]
async fn automatic_stalled_reopen_uses_the_qualified_release_and_refuses_a_held_target() {
    for (released, held) in [
        ("image-latest", false),
        ("image-queued", false),
        ("image-latest", true),
    ] {
        let (store, id) = stalled_reopen_fixture();
        if held {
            store.lock().held_runtime_target = Some(released.into());
        }
        let store = Arc::new(store);
        let before = store.get_sandbox(&id).await.unwrap().unwrap();
        let service = SandboxRuntimeUpdateService::new(store.clone(), released.into())
            .with_unpublished_recovery(Arc::new(VerifyOriginalProcesses));
        let result = service.recover_failed_unpublished(&id).await;
        if held {
            assert!(matches!(
                result,
                Err(crate::sandbox::SandboxRuntimeUpdateError::NotAvailable)
            ));
            assert_eq!(store.get_sandbox(&id).await.unwrap().unwrap(), before);
            assert!(!store.lock().sandbox_events.contains_key(&id));
        } else {
            let after = result.unwrap();
            assert_eq!(after.admitted_runtime_update_target(), Some(released));
            assert!(after.runtime_update_request().unwrap().automatic);
        }
    }
}

#[tokio::test]
async fn stalled_reopen_service_refuses_current_generation_log_publication_and_native_failure() {
    for case in 0..3 {
        let (store, id) = stalled_reopen_fixture();
        let store = Arc::new(store);
        let before = store.get_sandbox(&id).await.unwrap().unwrap();
        if case < 2 {
            let mut state = store.lock();
            state.agent_logs.values_mut().next().unwrap()[0].kind = if case == 0 {
                AgentLogKind::ConversationOpened {
                    conversation_id: "native-original".into(),
                    runtime_generation: 16,
                }
            } else {
                AgentLogKind::RuntimeGenerationRetired {
                    previous_generation: 15,
                    runtime_generation: 16,
                }
            };
        }
        let verifier: Arc<dyn SandboxUnpublishedRecoveryVerifier> = if case == 2 {
            Arc::new(RefuseUnverifiableProcess)
        } else {
            Arc::new(VerifyOriginalProcesses)
        };
        let service = SandboxRuntimeUpdateService::new(store.clone(), "image-latest".into())
            .with_unpublished_recovery(verifier);
        let caller = crate::Caller {
            subject: crate::SubjectId("operator".into()),
            authorization: crate::Authorization::new([crate::Capability::ControlAnySandbox]),
        };
        assert!(service
            .request_as_operator(&caller, &id, "image-latest")
            .await
            .is_err());
        assert_eq!(store.get_sandbox(&id).await.unwrap().unwrap(), before);
        assert!(!store.lock().sandbox_events.contains_key(&id));
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
        released_runtime: None,
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
        released_runtime: None,
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
                    released_runtime: None,
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

#[path = "unpublished_recovery_continuation_tests.rs"]
mod continuation;
use continuation::{RefuseUnverifiableProcess, VerifyOriginalProcesses};

#[path = "unpublished_recovery_parked_tests.rs"]
mod parked;
