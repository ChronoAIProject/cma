//! Real transactions prove the fence spans owners and independent store handles.
use super::*;

fn owned(index: usize) -> SandboxAdmission {
    let mut row = sandbox(
        &format!("sbx_global_{index}"),
        &format!("launch-global-{index}"),
    );
    row.owner = SubjectId::new(format!("owner-{index}"));
    row.name = "global".into();
    SandboxAdmission::new(
        row,
        SandboxAdmissionName::Allocate("global".into()),
        1,
        NOW.into(),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn different_owners_and_store_handles_cannot_overshoot_global_capacity() {
    let (client, database) = database("global_concurrent").await;
    let peer = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let left = MongoSandboxStore::open(&client, &database)
        .await
        .unwrap()
        .with_global_sandbox_limit(3);
    let right = MongoSandboxStore::open(&peer, &database)
        .await
        .unwrap()
        .with_global_sandbox_limit(3);
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(12));
    let results = futures_util::future::join_all((0..12).map(|index| {
        let store = if index % 2 == 0 {
            left.clone()
        } else {
            right.clone()
        };
        let barrier = barrier.clone();
        async move {
            barrier.wait().await;
            (index, store.admit_sandbox(owned(index)).await)
        }
    }))
    .await;
    let mut winners = Vec::new();
    let mut refused = 0;
    for (index, result) in results {
        match result {
            Ok(SandboxAdmissionResult::Created(value)) => winners.push((index, value)),
            Err(SandboxStoreError::GlobalCapacityReached) => refused += 1,
            other => panic!("unexpected admission outcome: {other:?}"),
        }
    }
    assert_eq!(winners.len(), 3);
    assert_eq!(refused, 9);
    assert_eq!(left.sandboxes.count_documents(doc! {}).await.unwrap(), 3);
    assert_eq!(
        left.sandbox_events.count_documents(doc! {}).await.unwrap(),
        3
    );
    // Startup must accept the durable fence and recover capacity without a counter.
    let restarted = MongoSandboxStore::open(&peer, &database)
        .await
        .unwrap()
        .with_global_sandbox_limit(3);
    for (index, sandbox) in winners {
        assert_eq!(
            restarted.admit_sandbox(owned(index)).await.unwrap(),
            SandboxAdmissionResult::Existing(sandbox)
        );
    }
    assert_eq!(
        restarted.admit_sandbox(owned(99)).await.unwrap_err(),
        SandboxStoreError::GlobalCapacityReached
    );
    assert_eq!(left.sandboxes.count_documents(doc! {}).await.unwrap(), 3);
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn global_capacity_counts_legacy_and_paused_rows_until_retirement() {
    let (client, database) = database("global_legacy").await;
    let legacy = MongoSandboxStore::open(&client, &database).await.unwrap();
    let mut prior = owned(0).sandbox;
    prior.origin = SandboxOrigin::Manual;
    prior.status = SandboxStatus::Paused;
    prior.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    prior.lifecycle.checkpoints.latest = Some(
        layer2_core::sandbox::SandboxCheckpointRef::seal(
            &layer3_adapter::CheckpointReceipt {
                epoch: 1,
                object_reference: "checkpoints/global-fixture".into(),
                sha256: "b".repeat(64),
                writers: layer3_adapter::WriterState::Held,
            },
            &[],
            NOW,
        )
        .unwrap(),
    );
    prior.validate().unwrap();
    legacy.create_sandbox(prior.clone()).await.unwrap();
    let store = legacy.clone().with_global_sandbox_limit(1);
    assert_eq!(
        store.admit_sandbox(owned(1)).await.unwrap_err(),
        SandboxStoreError::GlobalCapacityReached
    );
    let mut unadmitted = owned(2).sandbox;
    unadmitted.origin = SandboxOrigin::Manual;
    assert!(matches!(
        store.create_sandbox(unadmitted).await,
        Err(SandboxStoreError::InvalidTransaction(_))
    ));
    assert_eq!(
        store.sandbox_events.count_documents(doc! {}).await.unwrap(),
        0
    );

    // A supported terminal save releases the reservation; pause did not.
    prior.status = SandboxStatus::Retired;
    prior.retired_at = Some(LATER.into());
    prior.purge_after = Some(LATER.into());
    prior.work = SandboxWorkState::new(SandboxWorkStep::Purge, LATER.into());
    prior.updated_at = LATER.into();
    prior.revision += 1;
    store.save_sandbox(1, prior).await.unwrap();
    assert!(matches!(
        store.admit_sandbox(owned(1)).await.unwrap(),
        SandboxAdmissionResult::Created(_)
    ));
    assert_eq!(
        store.admit_sandbox(owned(2)).await.unwrap_err(),
        SandboxStoreError::GlobalCapacityReached
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn global_capacity_query_failure_rolls_back_and_does_not_claim_resources() {
    let (client, database) = database("global_failure").await;
    let store = MongoSandboxStore::open(&client, &database)
        .await
        .unwrap()
        .with_global_sandbox_limit(1);
    store
        .sandboxes
        .drop_index("sandboxes_global_live")
        .await
        .unwrap();
    assert!(matches!(
        store.admit_sandbox(owned(0)).await,
        Err(SandboxStoreError::Unavailable(_))
    ));
    assert_eq!(store.sandboxes.count_documents(doc! {}).await.unwrap(), 0);
    assert_eq!(
        store.sandbox_events.count_documents(doc! {}).await.unwrap(),
        0
    );
    assert_eq!(
        store
            .sandbox_admissions
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    // Default-off neither uses the global fence nor requires its new index.
    assert!(matches!(
        store
            .clone()
            .with_global_sandbox_limit(0)
            .admit_sandbox(owned(0))
            .await
            .unwrap(),
        SandboxAdmissionResult::Created(_)
    ));
    assert_eq!(
        store
            .sandbox_admissions
            .count_documents(doc! { "_id": { "scope": "global_sandbox_capacity" } })
            .await
            .unwrap(),
        0
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn owner_limit_and_failed_insert_do_not_spend_global_capacity() {
    let (client, database) = database("global_owner_limit").await;
    let store = MongoSandboxStore::open(&client, &database)
        .await
        .unwrap()
        .with_global_sandbox_limit(2);
    let mut denied = owned(0);
    denied.live_limit = 0;
    assert_eq!(
        store.admit_sandbox(denied).await.unwrap_err(),
        SandboxStoreError::LimitReached {
            limit: 0,
            live: 0,
            usage: Default::default()
        }
    );
    let first = owned(0);
    store.admit_sandbox(first.clone()).await.unwrap();
    let mut duplicate = owned(1);
    duplicate.sandbox.id = first.sandbox.id;
    assert_eq!(
        store.admit_sandbox(duplicate).await.unwrap_err(),
        SandboxStoreError::Duplicate
    );
    assert!(matches!(
        store.admit_sandbox(owned(2)).await.unwrap(),
        SandboxAdmissionResult::Created(_)
    ));
    assert_eq!(store.sandboxes.count_documents(doc! {}).await.unwrap(), 2);
    assert_eq!(
        store.sandbox_events.count_documents(doc! {}).await.unwrap(),
        2
    );
    client.database(&database).drop().await.unwrap();
}
