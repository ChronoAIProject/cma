use super::*;
use crate::sandbox::{
    SandboxImageRotationPhase, SandboxLifecycleOperation, SandboxRotationIdentity,
};

fn parked() -> (Arc<InMemorySandboxStore>, SandboxId) {
    let (store, id) = fixture();
    {
        let mut state = store.lock();
        let agents = state.agents.values().cloned().collect::<Vec<_>>();
        let row = state.sandboxes.get_mut(&id).unwrap();
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
            let (store, id) = parked();
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
