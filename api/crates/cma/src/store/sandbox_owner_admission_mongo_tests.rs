//! Per-owner limits survive independent managers and fresh process state.
use super::*;
use mongodb::bson::Bson;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn one_settled_retirement_slot_is_reused_once_across_managers() {
    for debt in [
        RetirementDebt::Missing,
        RetirementDebt::Empty,
        RetirementDebt::Outstanding,
    ] {
        exercise_retirement_admission(debt).await;
    }
}

#[derive(Clone, Copy)]
enum RetirementDebt {
    Missing,
    Empty,
    Outstanding,
}

async fn exercise_retirement_admission(debt: RetirementDebt) {
    use layer2_core::sandbox::{
        SandboxHome, SandboxLifecycleResult, SandboxStatus, SandboxWorkStep,
    };

    let (client, name) = database("owner_retirement_slot").await;
    let left = MongoSandboxStore::open(&client, &name).await.unwrap();
    let right = MongoSandboxStore::open(&client, &name).await.unwrap();
    let release_expected = !matches!(debt, RetirementDebt::Outstanding);
    for i in 0..2 {
        let id = format!("sbx_outgoing_{i}");
        let SandboxAdmissionResult::Created(mut parent) = left
            .admit_sandbox(admission(&id, &format!("launch-outgoing-{i}"), 2))
            .await
            .unwrap()
        else {
            panic!("expected new Sandbox");
        };
        let previous = parent.revision;
        parent.begin_retirement(NOW.into()).unwrap();
        if i == 0 {
            let mut home = SandboxHome::reserve(
                &parent.id,
                layer3_adapter::HomeVolumePolicy {
                    mode: Default::default(),
                    storage_class: "storage".into(),
                    capacity_gib: 64,
                    template_name: "empty".into(),
                    template_generation: "seed-generation".into(),
                },
            )
            .unwrap();
            home.released = true;
            parent.lifecycle.home = Some(Box::new(home));
            parent
                .work
                .schedule(SandboxWorkStep::MarkRetired, NOW.into());
            if !release_expected {
                parent
                    .credential
                    .queue_revoke("fixture-key".into(), NOW.into());
            }
        } else {
            assert!(!parent.retirement_released_owner_slot());
        }
        left.save_sandbox(previous, parent).await.unwrap();
    }
    if matches!(debt, RetirementDebt::Missing) {
        assert_eq!(
            left.sandboxes
                .update_one(
                    doc! {"_id": "sbx_outgoing_0"},
                    doc! {"$unset": {"record.credential.revoke_debt": ""}},
                )
                .await
                .unwrap()
                .modified_count,
            1,
        );
    }
    let retained = right
        .get_sandbox(&SandboxId("sbx_outgoing_0".into()))
        .await
        .unwrap()
        .unwrap();
    let lifecycle =
        SandboxLifecycleResult::observe(&retained, Some(NOW), std::time::SystemTime::now(), true)
            .unwrap();
    assert_eq!(lifecycle.owner_slot_released, Some(release_expected));

    let owner = SubjectId::new("owner");
    let snapshot = right.owner_capacity(&owner).await.unwrap();
    let released = u64::from(release_expected);
    assert_eq!(snapshot.counts, [(SandboxStatus::Retiring, 2 - released)]);
    assert_eq!(snapshot.retiring_released, released);
    let (a, b) = tokio::join!(
        left.admit_sandbox(admission("sbx_replacement_a", "launch-replacement-a", 2)),
        right.admit_sandbox(admission("sbx_replacement_b", "launch-replacement-b", 2)),
    );
    let results = [a, b];
    assert_eq!(
        results
            .iter()
            .filter(|value| matches!(value, Ok(SandboxAdmissionResult::Created(_))))
            .count(),
        usize::from(release_expected)
    );
    assert_eq!(
        results
            .iter()
            .filter(|value| matches!(
                value,
                Err(SandboxStoreError::LimitReached { limit: 2, live: 2, usage })
                    if usage.retiring_charged == 2 - released && usage.retiring_released == released
            ))
            .count(),
        2 - usize::from(release_expected),
    );
    client.database(&name).drop().await.unwrap();
}

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
            Err(SandboxStoreError::LimitReached {
                limit: 3, live: 3, ..
            }) => refused += 1,
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
        Err(SandboxStoreError::LimitReached {
            limit: 3,
            live: 3,
            ..
        })
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
    let (client, name) = database("owner_capacity").await;
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
