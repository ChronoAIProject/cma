use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn storage_activation_excludes_old_ready_environments_before_reconciliation() {
    let (client, name, mut store, _, profile, clock) = fixture(1).await;
    reconcile(&store, &profile, clock).await;
    let old = make_ready(&store, clock).await;
    store.warm_home_policy = Some(layer3_adapter::HomeVolumePolicy {
        mode: Default::default(),
        storage_class: "cma-home-v1".into(),
        capacity_gib: 64,
        template_name: "seed".into(),
        template_generation: "original-seed".into(),
    });
    let SandboxAdmissionResult::Created(fresh) = store
        .admit_sandbox(admission(
            "sbx_storage_cold",
            "launch-storage-cold",
            1,
            clock,
        ))
        .await
        .unwrap()
    else {
        panic!("created")
    };
    assert_ne!(fresh.id, old);
    assert!(fresh.runtime.is_none());
    assert_eq!(
        store.warm_sandbox(&old).await.unwrap().unwrap().state,
        WarmSandboxState::Ready
    );
    reconcile(&store, &profile, clock).await;
    assert_eq!(
        store.warm_sandbox(&old).await.unwrap().unwrap().state,
        WarmSandboxState::Retiring
    );
    assert_eq!(
        records(&store).await.len(),
        1,
        "replacement waits for old runtime cleanup"
    );
    client.database(&name).drop().await.unwrap();
}
