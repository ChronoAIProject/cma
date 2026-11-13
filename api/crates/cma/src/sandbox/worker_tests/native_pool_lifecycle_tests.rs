use super::*;

#[tokio::test]
async fn native_claim_restart_replays_every_persisted_stage_and_pins_default_image() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let pools = pools(&harness, &["claim", "observe", "adopt"]);
    let now = start_time(&created);
    for step in 0..12 {
        worker(&harness, pools.clone())
            .run_one(&created.id, now + Duration::from_secs(step * 31))
            .await
            .unwrap();
        let record = harness.stored(&created.id).await;
        if record.status == SandboxStatus::Active {
            break;
        }
    }
    let active = harness.stored(&created.id).await;
    assert_eq!(active.status, SandboxStatus::Active);
    let home = active.lifecycle.home.as_ref().unwrap();
    assert!(home.request.scope.is_some());
    assert_eq!(home.request.policy.template_name, "remote-seed");
    assert_eq!(home.request.policy.template_generation, "remote-seed-uid");
    assert_eq!(
        home.native_pool.as_ref().unwrap().request.home,
        home.request
    );
    assert_eq!(home.receipt.as_ref().unwrap().storage_generation, "pvc-uid");
    assert_eq!(
        active.runtime.as_ref().unwrap().runtime_version,
        home.native_pool
            .as_ref()
            .unwrap()
            .request
            .pool
            .spec
            .image
            .uri
    );
    assert_eq!(
        pools
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|v| **v == "select")
            .count(),
        1
    );
    assert!(pools.failures.lock().unwrap().is_empty());
}

#[tokio::test]
async fn native_retirement_recovers_unpublished_claim_and_disk_then_cleans_before_home_release() {
    for interruption in ["claim", "observe", "adopt"] {
        let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
        let created = harness.create().await;
        let pools = pools(&harness, &[interruption, "cleanup"]);
        let now = start_time(&created);
        worker(&harness, pools.clone())
            .run_one(&created.id, now)
            .await
            .unwrap();
        assert!(harness.stored(&created.id).await.runtime.is_none());
        harness
            .service
            .delete(
                &caller("owner", [Capability::DeleteOwnSandbox]),
                &created.id,
            )
            .await
            .unwrap();
        for step in 1..12 {
            worker(&harness, pools.clone())
                .run_one(&created.id, now + Duration::from_secs(step * 31))
                .await
                .unwrap();
            if harness.stored(&created.id).await.status == SandboxStatus::Retired {
                break;
            }
        }
        let retired = harness.stored(&created.id).await;
        assert_eq!(retired.status, SandboxStatus::Retired, "{interruption}");
        let home = retired.lifecycle.home.unwrap();
        assert!(home.released && home.native_pool.unwrap().cleaned);
        assert!(pools.claim.lock().unwrap().is_none());
        assert!(pools.disk.lock().unwrap().is_none());
        assert!(!harness
            .actions
            .snapshot()
            .iter()
            .any(|v| v.starts_with("mint:")));
    }
}

#[tokio::test]
async fn claim_created_after_deletion_loses_database_cas_and_is_reconciled_without_credentials() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let pools = pools(&harness, &[]);
    pools.delay_claim.store(true, Ordering::SeqCst);
    let now = start_time(&created);
    let slow = worker(&harness, pools.clone());
    let id = created.id.clone();
    let job = tokio::spawn(async move { slow.run_one(&id, now).await });
    tokio::time::timeout(Duration::from_secs(2), pools.entered.notified())
        .await
        .unwrap();
    harness
        .service
        .delete(
            &caller("owner", [Capability::DeleteOwnSandbox]),
            &created.id,
        )
        .await
        .unwrap();
    for step in 1..6 {
        worker(&harness, pools.clone())
            .run_one(&created.id, now + Duration::from_secs(step))
            .await
            .unwrap();
    }
    assert_eq!(
        harness.stored(&created.id).await.status,
        SandboxStatus::Retired
    );
    pools.resume.notify_one();
    assert!(matches!(
        job.await.unwrap(),
        Err(SandboxStoreError::Conflict)
    ));
    assert!(pools.claim.lock().unwrap().is_some());
    let backend = Backend {
        pools: pools.clone(),
        backend: harness.backend.clone(),
    };
    let adapter = Arc::new(
        CodexAdapter::with_backend(backend, CodexSandboxSpec::default())
            .with_home_volumes(pools.clone()),
    );
    crate::sandbox::reconciliation::SandboxReconciler::new(
        harness.store.clone(),
        adapter,
        Arc::new(harness.policy.clone()),
        "test".into(),
    )
    .initialize(now + Duration::from_secs(31))
    .await
    .unwrap();
    assert!(pools.claim.lock().unwrap().is_none());
    assert!(pools.calls.lock().unwrap().contains(&"reclaim"));
    assert!(!harness
        .actions
        .snapshot()
        .iter()
        .any(|v| v.starts_with("mint:")));
}
