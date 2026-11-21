use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn operator_correction_is_read_only_in_dry_run_refuses_stale_cas_and_audits_atomically() {
    let (seed, public, _) = setup("pending_operator").await;
    for index in 0..2 {
        seed.store
            .admit_launch(request(&public, index))
            .await
            .unwrap();
    }
    admissions(&seed)
        .update_one(fence_key(), doc! {"$set":{"pending_launch_homes":9_i64}})
        .await
        .unwrap();
    let (client, commands) = monitored_client().await;
    let registration =
        crate::store::MongoRegistrationCapacityStore::open(&client, &seed.database, false)
            .await
            .unwrap();
    let store = crate::store::MongoAdmissionCeilingsStore::open(&client, &seed.database, 0)
        .await
        .unwrap()
        .with_activation(registration.activation_observation())
        .with_pending_home_reconciliation(true);
    let settings = client
        .database(&seed.database)
        .collection::<Document>("platform_settings");
    let activity = client
        .database(&seed.database)
        .collection::<Document>("activity");
    let old_settings = settings.find_one(doc! {"_id":"platform"}).await.unwrap();
    let before = activity.count_documents(doc! {}).await.unwrap();
    let actor = SubjectId::new("reconciliation-operator");
    commands.lock().unwrap().clear();
    let dry = store
        .reconcile_pending_homes(8, &actor, "verify pending drift", true, 100)
        .await
        .unwrap();
    assert_eq!(
        (
            dry.previous_pending_launch_homes,
            dry.expected_pending_launch_homes,
            dry.pending_launch_homes
        ),
        (9, 2, 9)
    );
    assert!(!dry.corrected);
    assert!(writes(&commands.lock().unwrap()).is_empty());
    assert!(matches!(
        store
            .reconcile_pending_homes(8, &actor, "stale pending drift", false, 101)
            .await,
        Err(StoreError::RevisionConflict {
            expected: 8,
            actual: 9
        })
    ));
    assert!(writes(&commands.lock().unwrap()).is_empty());
    commands.lock().unwrap().clear();
    let fixed = store
        .reconcile_pending_homes(9, &actor, "reconcile verified receipts", false, 102)
        .await
        .unwrap();
    assert_eq!((fixed.pending_launch_homes, fixed.corrected), (2, true));
    assert_eq!(pending(&seed).await, 2);
    assert_eq!(activity.count_documents(doc! {}).await.unwrap(), before + 1);
    let evidence:Vec<Document>=settings.find(doc! {"_id":{"$gte":"pending_launch_homes_change:","$lt":"pending_launch_homes_change;"}})
        .await.unwrap().try_collect().await.unwrap();
    assert_eq!(evidence.len(), 1);
    let change = evidence[0].get_document("record").unwrap();
    assert_eq!(change.get_str("actor").unwrap(), actor.as_str());
    assert_eq!(
        change.get_str("reason").unwrap(),
        "reconcile verified receipts"
    );
    assert_eq!(change.get_i64("at").unwrap(), 102);
    assert_eq!(change.get_i64("old_pending_launch_homes").unwrap(), 9);
    assert_eq!(change.get_i64("new_pending_launch_homes").unwrap(), 2);
    assert_eq!(
        settings.find_one(doc! {"_id":"platform"}).await.unwrap(),
        old_settings
    );
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn unreadable_state_and_counter_underflow_never_release_a_receipt() {
    let (seed, public, reconciler) = setup("pending_corrupt").await;
    let lost = leak(&seed, &public, 0).await;
    admissions(&seed)
        .update_one(fence_key(), doc! {"$set":{"pending_launch_homes":0_i64}})
        .await
        .unwrap();
    assert_eq!(reconciler.sweep(None).await.unwrap().released, 0);
    assert_eq!(
        receipt(&seed, &lost).await.get_str("state").unwrap(),
        "reserved"
    );
    assert_eq!(pending(&seed).await, 0);
    let (client, commands) = monitored_client().await;
    let guarded = enabled(&client, &seed.database).await.unwrap();
    admissions(&seed)
        .update_one(
            doc! {"_id":{"scope":"registration_capacity"}},
            doc! {"$set":{"activated_at":"invalid"}},
        )
        .await
        .unwrap();
    commands.lock().unwrap().clear();
    assert!(guarded.sweep(None).await.is_err());
    assert!(guarded
        .correct(
            0,
            &SubjectId::new("operator"),
            "unreadable activation",
            false,
            100
        )
        .await
        .is_err());
    assert!(writes(&commands.lock().unwrap()).is_empty());
    assert_eq!(
        receipt(&seed, &lost).await.get_str("state").unwrap(),
        "reserved"
    );
    seed.client.database(&seed.database).drop().await.unwrap();
}
