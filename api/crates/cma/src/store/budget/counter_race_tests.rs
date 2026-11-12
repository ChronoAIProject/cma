//! Races that a collection of independent counters alone would not prevent.
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn distributed_counters_first_refusal_persists_stop_without_converting() {
    let (client, name, store, policy) = fixture(200, true).await;
    store
        .model_budget
        .update_one(
            doc! {"_id": period::period_key(policy.period_bounds().unwrap().0)},
            doc! {"$set": {"record.cloud.units": 200_i64}, "$inc": {"record.revision": 1_i64}},
        )
        .await
        .unwrap();
    assert!(matches!(
        reserve(&store, "refused", 1).await,
        Err(BudgetError::PlatformExhausted)
    ));
    let row = root(&store, &policy).await;
    let ledger: layer2_core::budget::PlatformPeriodLedger = decode(&row).unwrap();
    assert!(
        ledger
            .closures
            .iter()
            .any(|closure| closure.reason == layer2_core::budget::ClosureReason::SpendStop),
        "a refused first admission must preserve committed evidence for STOP persistence"
    );
    assert_eq!(
        store
            .model_budget
            .count_documents(doc! {"_id": {"$in": counters::keys(ledger.period_start)}})
            .await
            .unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn distributed_counters_fence_late_usage_and_cloud_corrections_against_admission() {
    for late_usage in [true, false] {
        let (client, name, store, policy) = fixture(200, true).await;
        let held = reserve(&store, "incurred", 100).await.unwrap();
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        let owner = layer2_core::SubjectId::new("new-commitment");
        let snapshot = period::read_snapshot(&store.budget_settings, Some(&owner), &mut session)
            .await
            .unwrap()
            .unwrap();
        let before = period::read_period(&store.model_budget, held.start, &mut session)
            .await
            .unwrap()
            .unwrap();
        assert!(before.ledger.committed(&policy, NOW).unwrap() < policy.period_cap);
        if late_usage {
            change(
                &store,
                held,
                Change {
                    retained: 150,
                    ..Default::default()
                },
            )
            .await;
        } else {
            store
                .record_cloud_observation(
                    CloudObservationInput {
                        observed_through: NOW + 1,
                        units: 150,
                    },
                    NOW + 1,
                )
                .await
                .unwrap();
        }
        // The stale snapshot still looks open. The mandatory settings write
        // must abort it before any counter on a different partition can grow.
        assert!(snapshot.platform.is_some());
        assert!(
            matches!(period::fenced_snapshot(&store, &owner, &mut session).await,
            Err(ApplyError::Driver(ref error)) if is_transient(error))
        );
        let _ = session.abort_transaction().await;
        let current = store.spend_state(NOW + 1).await.unwrap().ledger.unwrap();
        assert!(current.committed(&policy, NOW + 1).unwrap() > policy.period_cap);
        if late_usage {
            assert_eq!(
                current.reserved, 250,
                "incurred overage must remain recorded"
            );
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn distributed_counters_conversion_conflicts_with_an_old_writer_snapshot() {
    let (client, name, legacy, policy) = fixture(10_000, false).await;
    let held = reserve(&legacy, "legacy", 100).await.unwrap();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let mut old = period::read_period(&legacy.model_budget, held.start, &mut session)
        .await
        .unwrap()
        .unwrap();
    assert!(old.counters.is_none());
    let enabled = legacy.clone().with_platform_counter_shards(true);
    reserve(&enabled, "new", 100).await.unwrap();
    old.ledger.release(100).unwrap();
    assert!(
        matches!(period::save_period(&legacy, &old, &mut session).await,
        Err(ApplyError::Driver(ref error)) if is_transient(error))
    );
    let _ = session.abort_transaction().await;
    assert!(root(&legacy, &policy)
        .await
        .get_document("record")
        .unwrap()
        .contains_key("counter_layout"));
    assert_eq!(
        legacy
            .spend_state(NOW)
            .await
            .unwrap()
            .ledger
            .unwrap()
            .reserved,
        200
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn distributed_counters_aborted_conversion_leaves_no_partial_layout() {
    let (client, name, store, policy) = fixture(10_000, true).await;
    let before = root(&store, &policy).await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let owner = layer2_core::SubjectId::new("aborted");
    let snapshot = period::fenced_snapshot(&store, &owner, &mut session)
        .await
        .unwrap();
    let charge = period::admit_liability(&store, &snapshot, &owner, 100, NOW, &mut session)
        .await
        .unwrap()
        .unwrap();
    assert!(charge.shard.is_some());
    session.abort_transaction().await.unwrap();
    assert_eq!(root(&store, &policy).await, before);
    assert_eq!(
        store
            .model_budget
            .count_documents(doc! {
                "_id": {"$in": counters::keys(charge.start)},
            })
            .await
            .unwrap(),
        0
    );
    reserve(&store, "after-abort", 100).await.unwrap();
    assert_eq!(
        store
            .model_budget
            .count_documents(doc! {
                "_id": {"$in": counters::keys(charge.start)},
            })
            .await
            .unwrap(),
        u64::from(counters::COUNT)
    );
    client.database(&name).drop().await.unwrap();
}
