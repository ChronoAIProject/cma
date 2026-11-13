use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn native_prepared_concurrent_target_fence_caps_and_expiry_preserves_leases() {
    let (client, name, store) = database().await;
    let plan = plan();
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let (s, p) = (store.clone(), plan.clone());
        tasks.spawn(async move {
            s.reconcile_native_prepared_target(Some(&p), 2, NOW)
                .await
                .unwrap()
        });
    }
    while let Some(task) = tasks.join_next().await {
        task.unwrap();
    }
    assert_eq!(records(&store).await.len(), 2);
    let claim = store
        .claim_native_prepared_work(NOW, LATER)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        store
            .claim_native_prepared_work(NOW, LATER)
            .await
            .unwrap()
            .unwrap()
            .record
            .id,
        claim.record.id
    );
    assert!(store
        .claim_native_prepared_work(NOW, LATER)
        .await
        .unwrap()
        .is_none());
    store
        .reconcile_native_prepared_target(None, 0, NOW)
        .await
        .unwrap();
    assert!(store
        .finish_native_prepared_work(&claim, claim.record.clone(), NOW, NOW)
        .await
        .is_err());
    assert!(
        store
            .claim_native_prepared_work(NOW, LATER)
            .await
            .unwrap()
            .is_none(),
        "retirement waits original lease"
    );
    store
        .reconcile_native_prepared_target(Some(&plan), 2, EXPIRED)
        .await
        .unwrap();
    assert_eq!(
        records(&store).await.len(),
        2,
        "retirement occupies bounded capacity"
    );
    let cleanup = store
        .claim_native_prepared_work(EXPIRED, "2026-09-05T00:31:00Z")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cleanup.record.state, WarmSandboxState::Retiring);
    let mut retired = cleanup.record.clone();
    retired.state = WarmSandboxState::Retired;
    store
        .finish_native_prepared_work(&cleanup, retired, EXPIRED, EXPIRED)
        .await
        .unwrap();
    store
        .reconcile_native_prepared_target(Some(&plan), 2, EXPIRED)
        .await
        .unwrap();
    assert_eq!(
        records(&store)
            .await
            .iter()
            .filter(|r| r.payload.state.counts_toward_target())
            .count(),
        1
    );
    assert!(store
        .reconcile_native_prepared_target(Some(&plan), 3, NOW)
        .await
        .is_err());
    client.database(&name).drop().await.unwrap();
}
