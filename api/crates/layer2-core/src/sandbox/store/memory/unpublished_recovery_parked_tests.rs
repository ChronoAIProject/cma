use super::*;
use crate::sandbox::{
    SandboxImageRotationPhase, SandboxLifecycleOperation, SandboxRotationIdentity,
    SandboxRuntimeImageOrigin,
};

fn parked(machine_target: Option<&str>) -> (Arc<InMemorySandboxStore>, SandboxId) {
    let (mut store, id) = fixture();
    store.managed_machine_runtime_version = machine_target.map(str::to_owned);
    {
        let mut state = store.lock();
        let agents = state.agents.values().cloned().collect::<Vec<_>>();
        let row = state.sandboxes.get_mut(&id).unwrap();
        if machine_target.is_some() {
            row.origin = SandboxOrigin::Lifecycle {
                operation_id: "machine-creation".into(),
            };
            row.profile.content.runtime_image = Some("image-v1".into());
            row.profile.digest = crate::profiles::content_digest(&row.profile.content).unwrap();
            row.lifecycle.runtime_image_origin = Some(SandboxRuntimeImageOrigin::ManagedMachine);
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
    let (store, id) = parked(Some("image-v3"));
    let before = store.get_sandbox(&id).await.unwrap().unwrap();
    let released = Arc::new(layer3_adapter::PinnedReleasedRuntime::new(
        "unrelated-agent-release".into(),
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
