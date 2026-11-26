use super::*;
use layer2_core::sandbox::store::PageRequest;
use layer2_core::sandbox::{SandboxUnpublishedRecoveryRequest, SandboxUnpublishedRecoveryVerifier};

const TARGET: &str = "registry.example/runtime@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[tokio::test]
async fn legacy_missing_reservation_requires_independent_provider_request_and_quiet_worker() {
    for provider in [2, 3] {
        let (f, mut snapshot, _, now) = nonexecuting_recovery_tests::stalled(Some(provider)).await;
        let stamp = rfc3339_at(now);
        let fence = snapshot
            .sandbox
            .work
            .claim(
                "old-worker".into(),
                &stamp,
                rfc3339_at(now + Duration::from_secs(60)),
            )
            .unwrap();
        snapshot
            .sandbox
            .fail_lifecycle("legacy startup failed".into(), "old-worker", fence, stamp)
            .unwrap();
        snapshot.sandbox.lifecycle.failed_provision = None;
        let verifier = crate::sandbox::unpublished_recovery::UnpublishedRuntimeRecovery::new(
            f.adapter(TARGET),
            "test".into(),
        );
        // Never wait out a potentially live previous assignment or accept a stale
        // observation. A later caller must obtain a fresh complete snapshot.
        snapshot.sandbox.updated_at = layer2_core::now_rfc3339();
        assert!(verifier.verify(&snapshot).await.is_err());
        assert!(f.backend.inspected_generations.lock().unwrap().is_empty());
        snapshot.sandbox.updated_at = rfc3339_at(SystemTime::now() - Duration::from_secs(61));
        let result = verifier.verify(&snapshot).await;
        if provider == 2 {
            let proof = result.unwrap();
            assert_eq!(proof.observed_provision_generation, Some(2));
            assert!(proof.reserved_provision.is_none());
            proof.validate_for(&snapshot).unwrap();
        } else {
            assert!(result.is_err());
        }
        assert_eq!(*f.backend.inspected_generations.lock().unwrap(), vec![1, 2]);
        for action in ["suspend", "resume", "upgrade", "delete-retained"] {
            assert!(!f.backend.calls.snapshot().iter().any(|call| call == action));
        }
    }
}

#[tokio::test]
async fn reserved_provider_generation_recovers_same_home_after_lost_suspend_and_upgrade_replies() {
    let (f, snapshot, workspace, mut now) = nonexecuting_recovery_tests::stalled(Some(2)).await;
    let original = snapshot.sandbox.clone();
    let source = original.lifecycle.suspended_runtime.clone().unwrap();
    let reservation = original.lifecycle.runtime_provision.clone().unwrap();
    let before = f.backend.snapshot().await;
    let provider_before = f.backend.request().await;
    assert_eq!(source.provision_generation, Some(1));
    assert_eq!(reservation.generation, 2);
    assert_eq!(
        reservation.operation_id,
        original
            .lifecycle
            .operation
            .as_ref()
            .unwrap()
            .authority()
            .operation_id
    );
    let verifier = crate::sandbox::unpublished_recovery::UnpublishedRuntimeRecovery::new(
        f.adapter(TARGET),
        "test".into(),
    );
    let proof = verifier.verify(&snapshot).await.unwrap();
    assert_eq!(proof.runtime_generation, 1);
    assert_eq!(proof.reserved_provision.as_ref(), Some(&reservation));
    assert!(proof.boot_id.is_none());
    assert!(proof.runtime_process_absent);
    assert!(proof.conversations.is_empty());
    assert_eq!(*f.backend.inspected_generations.lock().unwrap(), vec![1, 2]);
    let admitted = f
        .harness
        .store
        .admit_unpublished_runtime_recovery(SandboxUnpublishedRecoveryRequest {
            snapshot,
            proof: proof.clone(),
            target_runtime_version: TARGET.into(),
            released_runtime: None,
            automatic: false,
            requested_at: rfc3339_at(now),
        })
        .await
        .unwrap();
    assert_eq!(admitted.lifecycle.suspended_runtime.as_ref(), Some(&source));
    assert_eq!(admitted.lifecycle.home, original.lifecycle.home);
    assert_eq!(admitted.lifecycle.runtime_generation_high_water, 2);
    assert!(admitted.lifecycle.runtime_provision.is_none());
    assert_eq!(
        admitted.retained_home_recovery().unwrap().source_generation,
        1
    );
    assert_eq!(
        crate::sandbox::runtime_attachment::retained_runtime_request(&admitted, &source, "test")
            .unwrap()
            .generation,
        1
    );
    assert_eq!(
        crate::sandbox::runtime_attachment::recovery_runtime_request(&admitted, &source, "test")
            .unwrap()
            .generation,
        2
    );

    now += Duration::from_secs(6);
    f.step(&admitted.id, now, TARGET).await;
    let quiesced = f.harness.stored(&admitted.id).await;
    let state = quiesced.lifecycle.unpublished_recovery.as_ref().unwrap();
    assert_eq!(state.proof.as_ref(), &proof);
    let receipt = state.quiescence.as_ref().unwrap();
    assert_eq!(receipt.runtime_generation, 1);
    assert_eq!(receipt.runtime_id, source.runtime_id);
    assert_eq!(receipt.provider_plan_sha256, proof.provider_plan_sha256);
    assert!(!f.backend.calls.snapshot().contains(&"suspend".into()));

    f.backend.lose_suspend.store(true, Ordering::SeqCst);
    f.backend.lose_resume.store(true, Ordering::SeqCst);
    now += Duration::from_secs(6);
    f.step(&admitted.id, now, TARGET).await;
    let lost_suspend = f.harness.stored(&admitted.id).await;
    assert!(!f.backend.lose_suspend.load(Ordering::SeqCst));
    assert!(lost_suspend.runtime.is_none());
    assert_eq!(
        lost_suspend.lifecycle.suspended_runtime.as_ref(),
        Some(&source)
    );
    assert_eq!(lost_suspend.lifecycle.home, original.lifecycle.home);
    assert!(lost_suspend.lifecycle.runtime_provision.is_none());
    assert_eq!(
        lost_suspend.lifecycle.unpublished_recovery,
        quiesced.lifecycle.unpublished_recovery
    );
    assert!(!f.backend.calls.snapshot().contains(&"upgrade".into()));

    let restored = f
        .drive(&admitted.id, &mut now, SandboxStatus::Active, TARGET)
        .await;
    let runtime = restored.runtime.as_ref().unwrap();
    assert_eq!(runtime.runtime_id, source.runtime_id);
    assert_eq!(runtime.provision_generation, Some(3));
    assert_eq!(runtime.runtime_version, TARGET);
    assert_eq!(restored.lifecycle.runtime_generation_high_water, 3);
    assert_eq!(restored.lifecycle.home, original.lifecycle.home);
    assert!(restored.lifecycle.suspended_runtime.is_none());
    assert!(restored.lifecycle.runtime_provision.is_none());
    assert!(!f.backend.lose_resume.load(Ordering::SeqCst));
    assert_eq!(
        f.harness
            .store
            .get_workspace(&workspace.id)
            .await
            .unwrap()
            .unwrap(),
        workspace
    );
    assert!(f
        .harness
        .store
        .list_agents(&restored.owner, &PageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
    let after = f.backend.snapshot().await;
    assert_eq!(after.workspaces, before.workspaces);
    assert_eq!(after.worktrees, before.worktrees);
    assert_eq!(after.homes, before.homes);
    assert!(after.agents.is_empty());
    let provider_after = f.backend.request().await;
    assert_eq!(provider_after.volumes, provider_before.volumes);
    assert_eq!(provider_after.metadata["cma/runtime-generation"], "3");
    let calls = f.backend.calls.snapshot();
    let suspend = calls.iter().position(|call| call == "suspend").unwrap();
    assert_eq!(calls[suspend - 1], "inspect-nonexecuting");
    let verified = calls
        .iter()
        .position(|call| call == "verify-suspended")
        .unwrap();
    let upgraded = calls.iter().position(|call| call == "upgrade").unwrap();
    assert!(suspend < verified && verified < upgraded);
    assert!(calls.contains(&"resume-replay".into()));
    assert_eq!(calls.iter().filter(|call| *call == "suspend").count(), 1);
    assert_eq!(calls.iter().filter(|call| *call == "resume").count(), 1);
    assert_eq!(calls.iter().filter(|call| *call == "create").count(), 1);
    assert!(!calls.contains(&"delete-retained".into()));
    assert!(f.backend.inspected_generations.lock().unwrap()[1..]
        .iter()
        .all(|generation| *generation == 2));
}

#[tokio::test]
async fn unreserved_provider_generation_cannot_be_admitted_or_suspend_the_retained_home() {
    let (f, snapshot, workspace, _) = nonexecuting_recovery_tests::stalled(Some(3)).await;
    let before = snapshot.sandbox.clone();
    let provider = f.backend.request().await;
    let verifier = crate::sandbox::unpublished_recovery::UnpublishedRuntimeRecovery::new(
        f.adapter(TARGET),
        "test".into(),
    );
    assert!(verifier.verify(&snapshot).await.is_err());
    assert_eq!(*f.backend.inspected_generations.lock().unwrap(), vec![1, 2]);
    assert_eq!(f.harness.stored(&before.id).await, before);
    assert_eq!(f.backend.request().await, provider);
    assert_eq!(
        f.harness
            .store
            .get_workspace(&workspace.id)
            .await
            .unwrap()
            .unwrap(),
        workspace
    );
    let calls = f.backend.calls.snapshot();
    for action in ["suspend", "resume", "upgrade", "delete-retained"] {
        assert!(!calls.iter().any(|call| call == action));
    }
    assert_eq!(calls.iter().filter(|call| *call == "create").count(), 1);
}
