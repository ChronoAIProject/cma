//! Per-owner limits survive independent managers and fresh process state.
use super::*;
use mongodb::bson::Bson;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn two_clients_enforce_three_sandboxes_per_owner_without_splitting_capacity() {
    let (client, name) = database("owner_three_replicas").await;
    let peer = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let left = MongoSandboxStore::open(&client, &name).await.unwrap();
    let right = MongoSandboxStore::open(&peer, &name).await.unwrap();
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(12));
    let results = futures_util::future::join_all((0..12).map(|index| {
        let store = if index % 2 == 0 {
            left.clone()
        } else {
            right.clone()
        };
        let barrier = barrier.clone();
        async move {
            let request = admission(
                &format!("sbx_owner_{index}"),
                &format!("launch-owner-{index}"),
                3,
            );
            barrier.wait().await;
            (request.clone(), store.admit_sandbox(request).await)
        }
    }))
    .await;
    let mut accepted = Vec::new();
    let mut refused = 0;
    for (request, result) in results {
        match result {
            Ok(SandboxAdmissionResult::Created(value)) => accepted.push((request, value)),
            Err(SandboxStoreError::LimitReached { limit: 3, live: 3 }) => refused += 1,
            other => panic!("unexpected shared owner admission: {other:?}"),
        }
    }
    assert_eq!(accepted.len(), 3);
    assert_eq!(refused, 9);
    assert_eq!(
        right
            .sandboxes
            .count_documents(doc! { "owner": "owner" })
            .await
            .unwrap(),
        3
    );
    assert_eq!(
        right.sandbox_events.count_documents(doc! {}).await.unwrap(),
        3
    );
    let snapshot = right
        .owner_capacity(&SubjectId::new("owner"))
        .await
        .unwrap();
    assert_eq!(
        snapshot.counts,
        vec![(layer2_core::sandbox::SandboxStatus::Provisioning, 3)]
    );
    // A new manager sees retained admission receipts even at full capacity.
    let restarted = MongoSandboxStore::open(&peer, &name).await.unwrap();
    for (request, value) in accepted {
        assert_eq!(
            restarted.admit_sandbox(request).await.unwrap(),
            SandboxAdmissionResult::Existing(value)
        );
    }
    assert!(matches!(
        restarted
            .admit_sandbox(admission("sbx_fourth", "launch-fourth", 3))
            .await,
        Err(SandboxStoreError::LimitReached { limit: 3, live: 3 })
    ));
    // Another owner's allowance remains independently available on either pod.
    let mut other = sandbox("sbx_other_owner", "launch-other-owner");
    other.owner = SubjectId("other-owner".into());
    other.name = "other".into();
    let other = SandboxAdmission::new(
        other,
        SandboxAdmissionName::Allocate("other".into()),
        3,
        NOW.into(),
    )
    .unwrap();
    assert!(matches!(
        left.admit_sandbox(other).await.unwrap(),
        SandboxAdmissionResult::Created(_)
    ));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn capacity_snapshot_counts_retiring_excludes_retired_and_refuses_corrupt_facts() {
    let (client, name) = database("owner_capacity_snapshot").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    for (index, status) in [
        "active", "paused", "failed", "rotating", "retiring", "retired",
    ]
    .iter()
    .enumerate()
    {
        let id = format!("sbx_capacity_{index}");
        store
            .admit_sandbox(admission(&id, &format!("launch-capacity-{index}"), 10))
            .await
            .unwrap();
        let retired = if *status == "retired" {
            Bson::String(NOW.into())
        } else {
            Bson::Null
        };
        store.sandboxes.update_one(doc! {"_id": &id}, doc! {"$set": {
            "status": status, "record.status": status, "retired_at": &retired, "record.retired_at": retired
        }}).await.unwrap();
    }
    let owner = SubjectId::new("owner");
    let snapshot = store.owner_capacity(&owner).await.unwrap();
    assert_eq!(
        snapshot.counts.iter().map(|(_, count)| count).sum::<u64>(),
        5
    );
    assert!(snapshot
        .counts
        .contains(&(layer2_core::sandbox::SandboxStatus::Retiring, 1)));
    assert!(store
        .owner_capacity(&SubjectId::new("other"))
        .await
        .unwrap()
        .counts
        .is_empty());
    for fields in [
        doc! {"status":"unknown", "record.status":"unknown"},
        doc! {"status":"retired", "record.status":"retired"},
        doc! {"status":"active", "record.status":"paused"},
        doc! {"status":"active", "record.status":"active", "document_schema_version": 99_i64},
    ] {
        store
            .sandboxes
            .update_one(doc! {"_id":"sbx_capacity_0"}, doc! {"$set":fields})
            .await
            .unwrap();
        assert!(matches!(
            store.owner_capacity(&owner).await,
            Err(SandboxStoreError::Corrupt(_))
        ));
    }
    client.database(&name).drop().await.unwrap();
}
