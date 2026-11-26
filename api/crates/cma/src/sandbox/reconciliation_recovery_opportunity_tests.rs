use super::super::super::super::recovery::Schedule;
use super::*;

#[tokio::test(start_paused = true)]
async fn custom_runtime_recovery_discovers_and_admits_the_recorded_pin() {
    let digest = |digit: &str| format!("registry.example/runtime@sha256:{}", digit.repeat(64));
    let source = digest("a");
    let target = digest("b");
    let managed = digest("c");
    for completed in [false, true] {
        let (store, reconciler) = setup(0, Duration::ZERO).await;
        let mut row = candidate(0);
        let failure = row.failure.take();
        row.status = SandboxStatus::Active;
        row.profile.content.runtime_image = Some(source.clone());
        row.profile.digest = layer2_core::profiles::content_digest(&row.profile.content).unwrap();
        row.runtime
            .as_mut()
            .unwrap()
            .runtime_version
            .clone_from(&source);
        let at = row.updated_at.clone();
        let migration = SandboxCustomRuntimeMigration {
            request_key: "migration-fixture".into(),
            expected_revision: row.revision,
            expected_image: source.clone(),
            target_image: target.clone(),
            origin: SandboxCustomRuntimeOrigin::capture(&row).unwrap(),
        };
        assert!(row
            .request_custom_runtime_migration(migration, &target, at.clone())
            .unwrap());
        let binding = row.runtime.as_mut().unwrap();
        binding.runtime_version.clone_from(&target);
        binding.provision_generation = Some(3);
        row.lifecycle.runtime_generation_high_water = 3;
        row.status = SandboxStatus::Failed;
        row.failure = failure;
        let update = row
            .lifecycle
            .runtime_update
            .as_mut()
            .unwrap()
            .request
            .as_mut()
            .unwrap();
        update.started_at = Some(at.clone());
        if completed {
            update.completed_at = Some(at.clone());
            let custom = row.lifecycle.custom_runtime.as_mut().unwrap();
            custom.completed_at = Some(at);
            custom.effective_image.clone_from(&target);
        }
        row.validate().unwrap();
        let id = row.id.clone();
        // Start from a restored migration row with no child records.
        *store.restored_recovery_row.lock().unwrap() = Some(row.clone());
        let verifier = Arc::new(Verify {
            calls: Mutex::default(),
            refuse: None,
        });
        let service = Arc::new(
            SandboxRuntimeUpdateService::new(store.clone(), managed.clone())
                .with_unpublished_recovery(verifier.clone()),
        );
        let caller = layer2_core::Caller {
            subject: row.owner.clone(),
            authorization: layer2_core::Authorization::new([
                layer2_core::Capability::UseOwnSandboxes,
            ]),
        };
        assert!(row.unpublished_recovery_candidate());
        assert!(!service.can_update_to(&row, &managed));
        assert!(matches!(
            service.request(&caller, &id, &target).await,
            Err(SandboxRuntimeUpdateError::NotAvailable)
        ));
        assert!(store.keys("recovery_snapshot").is_empty());
        assert!(store.keys("recovery_admit").is_empty());
        assert!(verifier.calls.lock().unwrap().is_empty());
        let reconciler = reconciler.with_failed_runtime_recovery(service);
        let mut lane = Schedule::default();
        lane.run(&reconciler).await.unwrap();
        assert!(verifier.calls.lock().unwrap().is_empty());
        lane.run(&reconciler).await.unwrap();
        assert_eq!(
            verifier.calls.lock().unwrap().as_slice(),
            std::slice::from_ref(&id)
        );
        assert_eq!(store.keys("recovery_admit"), [id.as_str()]);
        let recovered = store.restored_recovery_row.lock().unwrap().clone().unwrap();
        assert_eq!(
            recovered.admitted_runtime_update_target(),
            Some(target.as_str())
        );
        assert_eq!(
            recovered.lifecycle.custom_runtime,
            row.lifecycle.custom_runtime
        );
        assert!(recovered.lifecycle.unpublished_recovery_attempted);
    }
}

async fn slow_setup(
    successor: bool,
    refuse: bool,
) -> (Arc<fixtures::Store>, SandboxReconciler, Arc<Verify>) {
    // Four bounded pages take 16 seconds. The same fresh get/snapshot/admission
    // takes 12 seconds, so it fits a full opportunity but not discovery's tail.
    let (store, reconciler) = setup(6, Duration::from_secs(4)).await;
    store.create_sandbox(candidate(6)).await.unwrap();
    if successor {
        store.create_sandbox(candidate(7)).await.unwrap();
    }
    let verifier = Arc::new(Verify {
        calls: Mutex::default(),
        refuse: refuse.then(|| sandbox(6).id),
    });
    let service = Arc::new(
        SandboxRuntimeUpdateService::new(store.clone(), "fixture-image".into())
            .with_unpublished_recovery(verifier.clone()),
    );
    (
        store,
        reconciler.with_failed_runtime_recovery(service),
        verifier,
    )
}

#[tokio::test(start_paused = true)]
async fn slow_discovery_reserves_a_full_fitting_attempt_and_advances_or_wraps() {
    for successor in [false, true] {
        for refuse in [false, true] {
            let (store, reconciler, verifier) = slow_setup(successor, refuse).await;
            let before = store.inner.get_sandbox(&sandbox(6).id).await.unwrap();
            let mut lane = Schedule::default();
            let discovery = Instant::now();
            lane.run(&reconciler).await.unwrap();
            assert_eq!(discovery.elapsed(), Duration::from_secs(16));
            assert_eq!(
                store.keys("authority"),
                ["start", "sbx_001", "sbx_003", "sbx_005"]
            );
            assert!(store.keys("get").is_empty());
            assert!(verifier.calls.lock().unwrap().is_empty());
            assert!(store.keys("recovery_admit").is_empty());
            let opportunity = Instant::now();
            lane.run(&reconciler).await.unwrap();
            assert_eq!(
                opportunity.elapsed(),
                Duration::from_secs(if refuse { 8 } else { 12 })
            );
            assert_eq!(*verifier.calls.lock().unwrap(), [sandbox(6).id.clone()]);
            assert_eq!(store.keys("authority").len(), 4);
            assert_eq!(store.keys("recovery_admit").len(), usize::from(!refuse));
            if refuse {
                assert_eq!(
                    store.inner.get_sandbox(&sandbox(6).id).await.unwrap(),
                    before
                );
            } else {
                assert!(
                    store
                        .inner
                        .get_sandbox(&sandbox(6).id)
                        .await
                        .unwrap()
                        .unwrap()
                        .lifecycle
                        .unpublished_recovery_attempted
                );
            }

            // A refused first item cannot pin a successor. A terminal candidate
            // wraps fairly and, if admitted, cannot be automatically rearmed.
            lane.run(&reconciler).await.unwrap();
            assert_eq!(
                store.keys("authority")[4],
                if successor { "sbx_006" } else { "start" }
            );
            assert_eq!(verifier.calls.lock().unwrap().len(), 1);
            if successor || refuse {
                lane.run(&reconciler).await.unwrap();
                assert_eq!(
                    verifier.calls.lock().unwrap().last(),
                    Some(&sandbox(if successor { 7 } else { 6 }).id)
                );
            }
            assert!(store.keys("runtime_fence").is_empty());
            assert!(reconciler.progress.lock().await.is_none());
        }
    }
}

#[tokio::test(start_paused = true)]
async fn cancelled_slow_discovery_retries_only_the_unvalidated_page() {
    let (store, reconciler, verifier) = slow_setup(false, false).await;
    let mut lane = Schedule::default();
    assert!(
        tokio::time::timeout(Duration::from_secs(10), lane.run(&reconciler))
            .await
            .is_err()
    );
    assert!(verifier.calls.lock().unwrap().is_empty());
    assert!(store.keys("get").is_empty());
    let resumed = Instant::now();
    lane.run(&reconciler).await.unwrap();
    assert_eq!(resumed.elapsed(), Duration::from_secs(8));
    assert_eq!(
        store.keys("authority"),
        ["start", "sbx_001", "sbx_003", "sbx_003", "sbx_005"]
    );
    assert!(verifier.calls.lock().unwrap().is_empty());
    let opportunity = Instant::now();
    lane.run(&reconciler).await.unwrap();
    assert_eq!(opportunity.elapsed(), Duration::from_secs(12));
    assert_eq!(*verifier.calls.lock().unwrap(), [sandbox(6).id]);
    assert_eq!(store.keys("recovery_admit"), ["sbx_006"]);
    assert!(reconciler.progress.lock().await.is_none());
}

#[tokio::test(start_paused = true)]
async fn cancelled_fresh_attempt_advances_without_reusing_a_snapshot_or_proof() {
    // Cancel during the initial read and after proof while admission is pending.
    for seconds in [1, 10] {
        let (store, reconciler, verifier) = slow_setup(true, false).await;
        let before = store.inner.get_sandbox(&sandbox(6).id).await.unwrap();
        let mut lane = Schedule::default();
        lane.run(&reconciler).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(seconds), lane.run(&reconciler))
                .await
                .is_err()
        );
        assert_eq!(
            store.inner.get_sandbox(&sandbox(6).id).await.unwrap(),
            before
        );
        let first_proofs = usize::from(seconds == 10);
        assert_eq!(verifier.calls.lock().unwrap().len(), first_proofs);
        lane.run(&reconciler).await.unwrap();
        assert_eq!(store.keys("authority")[4], "sbx_006");
        lane.run(&reconciler).await.unwrap();
        assert_eq!(verifier.calls.lock().unwrap().last(), Some(&sandbox(7).id));
        lane.run(&reconciler).await.unwrap();
        let fresh = Instant::now();
        lane.run(&reconciler).await.unwrap();
        assert_eq!(fresh.elapsed(), Duration::from_secs(12));
        assert_eq!(store.keys("get"), ["sbx_006", "sbx_007", "sbx_006"]);
        assert_eq!(verifier.calls.lock().unwrap().len(), first_proofs + 2);
        assert_eq!(verifier.calls.lock().unwrap().last(), Some(&sandbox(6).id));
        assert!(store.keys("runtime_fence").is_empty());
    }
}

#[tokio::test(start_paused = true)]
async fn selected_id_rechecks_new_holds_and_current_eligibility_before_proof() {
    for held in [false, true] {
        let (store, reconciler, verifier) = slow_setup(true, false).await;
        let mut lane = Schedule::default();
        lane.run(&reconciler).await.unwrap();
        let mut current = store
            .inner
            .get_sandbox(&sandbox(6).id)
            .await
            .unwrap()
            .unwrap();
        if held {
            drop(reconciler.binding_holds.begin(&current, &[]).unwrap());
        } else {
            let revision = current.revision;
            current.revision += 1;
            current.failure.as_mut().unwrap().class = "terminal_validation".into();
            store
                .inner
                .save_sandbox(revision, current.clone())
                .await
                .unwrap();
        }
        lane.run(&reconciler).await.unwrap();
        assert!(verifier.calls.lock().unwrap().is_empty());
        // A new hold refuses before reads. A changed failure is rejected by
        // the complete fresh snapshot's predicate, before any native proof.
        assert_eq!(store.keys("recovery_snapshot").len(), usize::from(!held));
        assert!(store.keys("recovery_admit").is_empty());
        assert_eq!(store.keys("get").len(), usize::from(!held));
        assert_eq!(reconciler.binding_holds.contains(current.id.as_str()), held);
        assert_eq!(
            store.inner.get_sandbox(&current.id).await.unwrap(),
            Some(current)
        );
        lane.run(&reconciler).await.unwrap();
        lane.run(&reconciler).await.unwrap();
        assert_eq!(*verifier.calls.lock().unwrap(), [sandbox(7).id]);
        assert_eq!(store.keys("recovery_admit"), ["sbx_007"]);
        assert!(store.keys("runtime_fence").is_empty());
        assert!(reconciler.progress.lock().await.is_none());
    }
}
