use super::*;
use crate::sandbox::{
    SandboxImageRotationPhase, SandboxLifecycleOperation, SandboxRotationIdentity,
    SandboxRuntimeImageOrigin,
};
use layer3_adapter::{PinnedReleasedRuntime, ReleasedRuntimeSource};
use std::sync::atomic::{AtomicUsize, Ordering};

fn parked(origin: Option<SandboxRuntimeImageOrigin>) -> (Arc<InMemorySandboxStore>, SandboxId) {
    let (mut store, id) = fixture();
    if origin == Some(SandboxRuntimeImageOrigin::ManagedMachine) {
        store.managed_machine_runtime_version = Some("image-v3".into());
    }
    {
        let mut state = store.lock();
        let agents = state.agents.values().cloned().collect::<Vec<_>>();
        let row = state.sandboxes.get_mut(&id).unwrap();
        if let Some(origin) = origin {
            row.origin = SandboxOrigin::Lifecycle {
                operation_id: "managed-creation".into(),
            };
            row.profile.content.runtime_image = Some("image-v1".into());
            row.profile.digest = crate::profiles::content_digest(&row.profile.content).unwrap();
            row.lifecycle.runtime_image_origin = Some(origin);
        }
        row.status = SandboxStatus::Active;
        row.failure = None;
        row.request_runtime_update("image-v2".into(), false, NOW.into())
            .unwrap();
        let identity = SandboxRotationIdentity::capture(row, &agents).unwrap();
        row.begin_image_rotation("image-v2".into(), identity, NOW.into())
            .unwrap();
        let Some(SandboxLifecycleOperation::ImageRotation { phase, .. }) =
            &mut row.lifecycle.operation
        else {
            panic!()
        };
        *phase = SandboxImageRotationPhase::ConversationReopen;
        row.lifecycle.unpublished_recovery_attempted = true;
        let fence = row
            .work
            .claim("fixture".into(), NOW, "2026-09-05T01:00:00Z".into())
            .unwrap();
        row.park_failed_runtime_update(
            TypedFailure {
                class: "terminal_validation".into(),
                detail: "fixture terminal rejection".into(),
            },
            "fixture",
            fence,
            NOW.into(),
        )
        .unwrap();
        row.validate().unwrap();
    }
    (Arc::new(store), id)
}

#[tokio::test]
async fn consumed_parked_retry_requires_explicit_consent_and_exact_current_proof() {
    for operator in [false, true] {
        for refusal in [false, true] {
            let (store, id) = parked(None);
            let before = store
                .unpublished_runtime_recovery_snapshot(&id)
                .await
                .unwrap();
            before.validate().unwrap();
            let verifier: Arc<dyn SandboxUnpublishedRecoveryVerifier> = if refusal {
                Arc::new(RefuseUnverifiableProcess)
            } else {
                Arc::new(VerifyOriginalProcesses)
            };
            let service = SandboxRuntimeUpdateService::new(store.clone(), "image-v2".into())
                .with_unpublished_recovery(verifier);
            assert!(matches!(
                service.recover_failed_unpublished(&id).await,
                Err(crate::sandbox::SandboxRuntimeUpdateError::NotAvailable)
            ));
            let unauthorized = crate::Caller {
                subject: before.sandbox.owner.clone(),
                authorization: crate::Authorization::new([]),
            };
            assert!(matches!(
                service.request(&unauthorized, &id, "image-v2").await,
                Err(crate::sandbox::SandboxRuntimeUpdateError::Forbidden)
            ));
            let caller = crate::Caller {
                subject: before.sandbox.owner.clone(),
                authorization: crate::Authorization::new([if operator {
                    crate::Capability::ControlAnySandbox
                } else {
                    crate::Capability::UseOwnSandboxes
                }]),
            };
            let result = if operator {
                service.request_as_operator(&caller, &id, "image-v2").await
            } else {
                service.request(&caller, &id, "image-v2").await
            };
            if refusal {
                assert!(result.is_err());
                assert_eq!(
                    store.get_sandbox(&id).await.unwrap().unwrap(),
                    before.sandbox
                );
                assert!(!store.lock().sandbox_events.contains_key(&id));
                continue;
            }
            let after = result.unwrap();
            assert!(after.lifecycle.unpublished_recovery_attempted);
            assert!(!after.runtime_update_request().unwrap().automatic);
            assert_eq!(after.lifecycle.home, before.sandbox.lifecycle.home);
            assert_eq!(
                after.lifecycle.suspended_runtime.as_deref(),
                before.sandbox.runtime.as_ref()
            );
            assert_eq!(
                *after.lifecycle.unpublished_recovery.as_ref().unwrap().proof,
                proof(&before)
            );
            let replay = if operator {
                service.request_as_operator(&caller, &id, "image-v2").await
            } else {
                service.request(&caller, &id, "image-v2").await
            }
            .unwrap();
            assert_eq!(replay, after);
            assert_eq!(store.lock().sandbox_events[&id].len(), 1);
            assert_eq!(
                store.lock().agents.values().cloned().collect::<Vec<_>>(),
                before.agents
            );
        }
    }
}

#[derive(Default)]
struct CountingVerifier(AtomicUsize);

#[async_trait::async_trait]
impl SandboxUnpublishedRecoveryVerifier for CountingVerifier {
    async fn verify(
        &self,
        snapshot: &SandboxUnpublishedRecoverySnapshot,
    ) -> Result<SandboxUnpublishedRecoveryProof, SandboxStoreError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(proof(snapshot))
    }
}

#[tokio::test]
async fn held_release_cannot_admit_an_agent_reopen_until_unheld() {
    for (revive, operator, pinned, latest) in [
        (false, false, "image-v2", "image-v3"),
        (false, true, "image-v2", "image-v2"),
        (true, true, "image-queued", "image-v3"),
        (true, false, "image-queued", "image-queued"),
    ] {
        let (store, id) = if revive {
            let (store, id) = stalled_reopen_fixture();
            (Arc::new(store), id)
        } else {
            parked(None)
        };
        let before = store
            .unpublished_runtime_recovery_snapshot(&id)
            .await
            .unwrap();
        before.validate().unwrap();
        let logs = store.lock().agent_logs.clone();
        let verifier = Arc::new(CountingVerifier::default());
        let release = Arc::new(PinnedReleasedRuntime::new(
            latest,
            Some("machine-v1".into()),
        ));
        let service = SandboxRuntimeUpdateService::with_released(store.clone(), release.clone())
            .with_unpublished_recovery(verifier.clone());
        let caller = crate::Caller {
            subject: if operator {
                crate::SubjectId("operator".into())
            } else {
                before.sandbox.owner.clone()
            },
            authorization: crate::Authorization::new([if operator {
                crate::Capability::ControlAnySandbox
            } else {
                crate::Capability::UseOwnSandboxes
            }]),
        };
        let request = || async {
            if operator {
                service.request_as_operator(&caller, &id, pinned).await
            } else {
                service.request(&caller, &id, pinned).await
            }
        };

        store.lock().held_runtime_target = Some(latest.into());
        assert!(store.managed_runtime_target_is_held(latest));
        assert!(!service.target_is_qualified(&before.sandbox, latest));
        let can_update = service.can_update(&before.sandbox);
        let result = request()
            .await
            .map(|sandbox| sandbox.admitted_runtime_update_target().unwrap().to_owned());
        assert!(
            !can_update
                && matches!(result, Err(crate::sandbox::SandboxRuntimeUpdateError::NotAvailable)),
            "revive={revive}, operator={operator}, pinned={pinned}, latest={latest}: can_update={can_update}, request={result:?}"
        );
        assert!(!service.retry_uses_latest(&before.sandbox, pinned, latest));
        assert_eq!(verifier.0.load(Ordering::SeqCst), 0);
        assert_eq!(
            store
                .unpublished_runtime_recovery_snapshot(&id)
                .await
                .unwrap(),
            before
        );
        assert!(!store.lock().sandbox_events.contains_key(&id));
        assert_eq!(store.lock().agent_logs, logs);

        {
            let mut state = store.lock();
            state.held_runtime_target = None;
            state.expected_recovery_release = Some(release.snapshot());
        }
        assert!(service.can_update(&before.sandbox));
        assert_eq!(
            service.retry_uses_latest(&before.sandbox, pinned, latest),
            pinned != latest
        );
        let after = request().await.unwrap();
        assert_eq!(after.admitted_runtime_update_target(), Some(latest));
        assert_eq!(after.lifecycle.home, before.sandbox.lifecycle.home);
        let recovery = after.lifecycle.unpublished_recovery.as_ref().unwrap();
        assert_eq!(*recovery.proof, proof(&before));

        store.lock().held_runtime_target = Some(latest.into());
        let replay = if operator {
            service.request_as_operator(&caller, &id, latest).await
        } else {
            service.request(&caller, &id, latest).await
        };
        assert_eq!(replay.unwrap(), after);
        assert_eq!(verifier.0.load(Ordering::SeqCst), 1);
        let state = store.lock();
        assert!(matches!(
            state.sandbox_events[&id].as_slice(),
            [event] if matches!(
                &event.kind,
                SandboxEventKind::RuntimeRecoveryAdmitted { target_runtime_version, .. }
                    if target_runtime_version == latest
            )
        ));
        assert_eq!(
            state.agents.values().cloned().collect::<Vec<_>>(),
            before.agents
        );
        assert_eq!(state.agent_logs, logs);
    }
}

#[tokio::test]
async fn agent_reopen_requires_a_nonempty_and_qualified_release() {
    for (origin, latest) in [
        (None, ""),
        (Some(SandboxRuntimeImageOrigin::ManagedDesktop), "image-v3"),
    ] {
        let (store, id) = parked(origin);
        let before = store
            .unpublished_runtime_recovery_snapshot(&id)
            .await
            .unwrap();
        before.validate().unwrap();
        let verifier = Arc::new(CountingVerifier::default());
        let service = SandboxRuntimeUpdateService::new(store.clone(), latest.into())
            .with_unpublished_recovery(verifier.clone());
        let owner = crate::Caller {
            subject: before.sandbox.owner.clone(),
            authorization: crate::Authorization::new([crate::Capability::UseOwnSandboxes]),
        };
        assert!(!service.can_update(&before.sandbox));
        assert!(!service.retry_uses_latest(&before.sandbox, "image-v2", latest));
        assert!(matches!(
            service.request(&owner, &id, "image-v2").await,
            Err(crate::sandbox::SandboxRuntimeUpdateError::NotAvailable)
        ));
        assert_eq!(verifier.0.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn retrying_a_reopen_parked_on_a_superseded_target_recovers_onto_the_latest_runtime() {
    // #2257: the owner's Retry sends the pinned target after the latest runtime moved on.
    for operator in [false, true] {
        let (store, id) = parked(None);
        let before = store
            .unpublished_runtime_recovery_snapshot(&id)
            .await
            .unwrap();
        assert_eq!(
            before.sandbox.admitted_runtime_update_target(),
            Some("image-v2")
        );
        let service = SandboxRuntimeUpdateService::new(store.clone(), "image-v3".into())
            .with_unpublished_recovery(Arc::new(VerifyOriginalProcesses));
        assert!(service.retry_uses_latest(&before.sandbox, "image-v2", "image-v3"));
        let caller = crate::Caller {
            subject: before.sandbox.owner.clone(),
            authorization: crate::Authorization::new([if operator {
                crate::Capability::ControlAnySandbox
            } else {
                crate::Capability::UseOwnSandboxes
            }]),
        };
        let after = if operator {
            service.request_as_operator(&caller, &id, "image-v2").await
        } else {
            service.request(&caller, &id, "image-v2").await
        }
        .unwrap();
        assert_eq!(after.admitted_runtime_update_target(), Some("image-v3"));
        assert_eq!(
            after.runtime_update_request().unwrap().target_version,
            "image-v3"
        );
        assert!(after.lifecycle.unpublished_recovery_attempted);
        assert!(!after.runtime_update_request().unwrap().automatic);
        assert_eq!(after.lifecycle.home, before.sandbox.lifecycle.home);
        assert_eq!(
            store.lock().agents.values().cloned().collect::<Vec<_>>(),
            before.agents
        );
    }
}

#[tokio::test]
async fn retrying_a_machine_reopen_keeps_the_admitted_target_after_a_release() {
    let (store, id) = parked(Some(SandboxRuntimeImageOrigin::ManagedMachine));
    let before = store.get_sandbox(&id).await.unwrap().unwrap();
    let released = Arc::new(layer3_adapter::PinnedReleasedRuntime::new(
        "unrelated-agent-release",
        Some("image-v3".into()),
    ));
    let service = SandboxRuntimeUpdateService::with_released(store.clone(), released)
        .with_unpublished_recovery(Arc::new(VerifyOriginalProcesses));
    assert!(service.target_is_qualified(&before, "image-v3"));
    assert!(!service.retry_uses_latest(&before, "image-v2", "image-v3"));
    let caller = crate::Caller {
        subject: before.owner.clone(),
        authorization: crate::Authorization::new([crate::Capability::UseOwnSandboxes]),
    };
    let after = service.request(&caller, &id, "image-v2").await.unwrap();
    assert_eq!(after.admitted_runtime_update_target(), Some("image-v2"));
    assert_eq!(
        after.runtime_update_request().unwrap().target_version,
        "image-v2"
    );
    assert_eq!(after.lifecycle.home, before.lifecycle.home);
}

#[tokio::test]
async fn retrying_a_parked_update_before_reopen_keeps_its_pinned_target() {
    // Only a stalled reopen moves off a superseded target; other parked phases retry as pinned.
    let (store, id) = parked(None);
    {
        let mut state = store.lock();
        let row = state.sandboxes.get_mut(&id).unwrap();
        let Some(SandboxLifecycleOperation::ImageRotation { phase, .. }) =
            &mut row.lifecycle.operation
        else {
            panic!()
        };
        *phase = SandboxImageRotationPhase::HomeRestore;
        row.validate().unwrap();
    }
    let before = store.get_sandbox(&id).await.unwrap().unwrap();
    assert!(!before.unpublished_recovery_candidate());
    let service = SandboxRuntimeUpdateService::new(store.clone(), "image-v3".into())
        .with_unpublished_recovery(Arc::new(VerifyOriginalProcesses));
    let caller = crate::Caller {
        subject: before.owner.clone(),
        authorization: crate::Authorization::new([crate::Capability::UseOwnSandboxes]),
    };
    let after = service.request(&caller, &id, "image-v2").await.unwrap();
    assert_eq!(after.admitted_runtime_update_target(), Some("image-v2"));
    assert_eq!(
        after.runtime_update_request().unwrap().target_version,
        "image-v2"
    );
    assert!(after.runtime_update_request().unwrap().failure.is_none());
    assert!(after.lifecycle.unpublished_recovery.is_none());
}
