use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn legacy_retirement_preserves_live_lease_and_fences_stale_worker() {
    let (client, name, store, _, profile, clock) = fixture(1).await;
    reconcile(&store, &profile, clock).await;
    let old = store
        .claim_warmup_work(clock.stamp(0).as_str(), clock.stamp(60).as_str())
        .await
        .unwrap()
        .unwrap();
    assert!(store
        .claim_legacy_warm_cleanup(clock.stamp(0).as_str(), clock.stamp(60).as_str())
        .await
        .unwrap()
        .is_none());
    store
        .retire_legacy_warmups(clock.stamp(0).as_str())
        .await
        .unwrap();
    let retired = store.warm_sandbox(&old.record.id).await.unwrap().unwrap();
    assert_eq!(retired.state, WarmSandboxState::Retiring);
    assert_eq!(retired.revision, old.record.revision + 1);
    assert_eq!(retired.plan, old.record.plan);
    assert!(store
        .claim_legacy_warm_cleanup(clock.stamp(0).as_str(), clock.stamp(60).as_str())
        .await
        .unwrap()
        .is_none());
    assert!(matches!(
        store
            .renew_warmup_work(&old, clock.stamp(60).as_str())
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    assert!(matches!(
        store
            .finish_warmup_work(
                &old,
                old.record.clone(),
                clock.stamp(0).as_str(),
                clock.stamp(0).as_str()
            )
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    // A restart/retry neither creates a replacement nor changes authority again.
    store
        .retire_legacy_warmups(clock.stamp(0).as_str())
        .await
        .unwrap();
    assert_eq!(
        records(&store).await.as_slice(),
        std::slice::from_ref(&retired)
    );
    expire(&store, &old.record.id).await;
    let cleanup = store
        .claim_legacy_warm_cleanup(clock.stamp(60).as_str(), clock.stamp(120).as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cleanup.record, retired);
    let mut next = cleanup.record.clone();
    next.state = WarmSandboxState::Retired;
    store
        .finish_warmup_work(
            &cleanup,
            next,
            clock.stamp(60).as_str(),
            clock.stamp(60).as_str(),
        )
        .await
        .unwrap();
    store
        .retire_legacy_warmups(clock.stamp(60).as_str())
        .await
        .unwrap();
    assert!(store
        .claim_legacy_warm_cleanup(clock.stamp(60).as_str(), clock.stamp(120).as_str())
        .await
        .unwrap()
        .is_none());
    assert_eq!(records(&store).await.len(), 1);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn default_store_admission_never_claims_legacy_ready_rows_and_cleanup_keeps_claimed_rows() {
    let (client, name, fixture_store, _, profile, clock) = fixture(2).await;
    reconcile(&fixture_store, &profile, clock).await;
    let _ = make_ready(&fixture_store, clock).await;
    let _ = make_ready(&fixture_store, clock).await;
    // Simulate the old version's already committed ownership transfer.
    let SandboxAdmissionResult::Created(claimed) = fixture_store
        .admit_sandbox(admission("sbx_old", "old-version-handoff", 10, clock))
        .await
        .unwrap()
    else {
        panic!("expected historical admission")
    };
    let original = fixture_store
        .warm_sandbox(&claimed.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(original.state, WarmSandboxState::Claimed);

    // Production opens this default state and no longer configures a legacy pin.
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert!(store.warm_runtime_version.is_none());
    let SandboxAdmissionResult::Created(fresh) = store
        .admit_sandbox(admission("sbx_new", "new-version-admission", 10, clock))
        .await
        .unwrap()
    else {
        panic!("expected native lifecycle admission")
    };
    assert_eq!(fresh.id.as_str(), "sbx_new");
    assert!(fresh.runtime.is_none());
    store
        .retire_legacy_warmups(clock.stamp(0).as_str())
        .await
        .unwrap();
    assert_eq!(
        store.warm_sandbox(&claimed.id).await.unwrap(),
        Some(original)
    );
    let values = records(&store).await;
    assert_eq!(values.len(), 2);
    assert_eq!(
        values
            .iter()
            .filter(|row| row.state == WarmSandboxState::Retiring)
            .count(),
        1
    );
    assert_eq!(store.get_sandbox(&claimed.id).await.unwrap(), Some(claimed));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn concurrent_legacy_retirement_and_old_admission_never_share_cleanup_authority() {
    let (client, name, store, _, profile, clock) = fixture(1).await;
    reconcile(&store, &profile, clock).await;
    let id = make_ready(&store, clock).await;
    let at = clock.stamp(0);
    let (retirement, admission) = tokio::join!(
        store.retire_legacy_warmups(&at),
        store.admit_sandbox(admission("sbx_racing", "race-handoff", 10, clock))
    );
    retirement.unwrap();
    let SandboxAdmissionResult::Created(created) = admission.unwrap() else {
        panic!("expected creation")
    };
    let old = store.warm_sandbox(&id).await.unwrap().unwrap();
    if created.id == id {
        assert_eq!(old.state, WarmSandboxState::Claimed);
        assert!(store
            .claim_legacy_warm_cleanup(clock.stamp(60).as_str(), clock.stamp(120).as_str())
            .await
            .unwrap()
            .is_none());
    } else {
        assert_eq!(old.state, WarmSandboxState::Retiring);
        assert!(store.get_sandbox(&id).await.unwrap().is_none());
        assert_eq!(
            store
                .claim_legacy_warm_cleanup(clock.stamp(60).as_str(), clock.stamp(120).as_str())
                .await
                .unwrap()
                .unwrap()
                .record
                .id,
            id
        );
    }
    client.database(&name).drop().await.unwrap();
}
