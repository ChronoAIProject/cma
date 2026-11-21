use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn terminal_leaks_release_once_on_two_replicas_without_touching_missing_or_live_launches() {
    let (seed, public, first) = setup("pending_repair").await;
    let lost = leak(&seed, &public, 0).await;
    let live = seed
        .store
        .admit_launch(request(&public, 1))
        .await
        .unwrap()
        .launch()
        .clone();
    let missing = "lch_missing_pending";
    admissions(&seed)
        .insert_one(
            doc! {"_id":{"scope":"launch_home_capacity","launch":missing},
            "document_schema_version":1_i64,"owner":"missing-owner","state":"reserved"},
        )
        .await
        .unwrap();
    admissions(&seed)
        .update_one(fence_key(), doc! {"$inc":{"pending_launch_homes":1_i64}})
        .await
        .unwrap();
    let second = enabled(&seed.client, &seed.database).await.unwrap();
    let (a, b) = tokio::join!(first.sweep(None), second.sweep(None));
    assert_eq!(a.unwrap().released + b.unwrap().released, 1);
    assert_eq!(pending(&seed).await, 2);
    assert_eq!(
        receipt(&seed, &lost).await.get_str("state").unwrap(),
        "released"
    );
    assert_eq!(
        receipt(&seed, &live).await.get_str("state").unwrap(),
        "reserved"
    );
    assert_eq!(
        admissions(&seed)
            .find_one(receipt_key(missing))
            .await
            .unwrap()
            .unwrap()
            .get_str("state")
            .unwrap(),
        "reserved"
    );
    let report = first.sweep(None).await.unwrap();
    assert_eq!(report.released, 0);
    assert_eq!(report.missing_launches, 1);
    assert_eq!(first.observe().await.unwrap().drift, 0);
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn repair_races_terminal_release_reservation_and_sandbox_transfer_without_drift() {
    let (seed, public, first) = setup("pending_races").await;
    let lost = leak(&seed, &public, 0).await;
    let normal = seed
        .store
        .admit_launch(request(&public, 1))
        .await
        .unwrap()
        .launch()
        .clone();
    let transfer = seed
        .store
        .admit_launch(request(&public, 2))
        .await
        .unwrap()
        .launch()
        .clone();
    let other =
        MongoTriggerDefinitionStore::open(&seed.client, &seed.database, seed.profiles.clone())
            .await
            .unwrap();
    let resource = resources(&seed).await;
    let second = enabled(&seed.client, &seed.database).await.unwrap();
    let home = home(&seed, &transfer, "sbx_pending_transfer", false);
    let new = request(&public, 3);
    let (a, b, c, d, e) = tokio::join!(
        first.sweep(None),
        second.sweep(None),
        other.commit_launch(cancel_commit(normal.clone())),
        other.admit_launch(new),
        resource.admit_sandbox(home)
    );
    assert_eq!(a.unwrap().released + b.unwrap().released, 1);
    c.unwrap();
    d.unwrap();
    e.unwrap();
    assert_eq!(
        receipt(&seed, &lost).await.get_str("state").unwrap(),
        "released"
    );
    assert_eq!(
        receipt(&seed, &normal).await.get_str("state").unwrap(),
        "released"
    );
    assert_eq!(
        receipt(&seed, &transfer).await.get_str("state").unwrap(),
        "consumed"
    );
    let counts = first.observe().await.unwrap();
    assert_eq!((counts.pending, counts.expected, counts.drift), (1, 1, 0));
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn completed_traversal_revisits_previously_missing_terminal_evidence() {
    let (seed, public, reconciler) = setup("pending_revisit").await;
    let lost = leak(&seed, &public, 0).await;
    let launches = seed
        .client
        .database(&seed.database)
        .collection::<Document>(LAUNCHES);
    let row = launches
        .find_one(doc! {"_id":lost.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    launches
        .delete_one(doc! {"_id":lost.id.as_str()})
        .await
        .unwrap();
    let first = reconciler.sweep(None).await.unwrap();
    assert_eq!((first.released, first.missing_launches), (0, 1));
    assert_eq!(first.next, None);
    launches.insert_one(row).await.unwrap();
    let second = reconciler.sweep(first.next).await.unwrap();
    assert_eq!(second.released, 1);
    assert_eq!(pending(&seed).await, 0);
    assert_eq!(
        receipt(&seed, &lost).await.get_str("state").unwrap(),
        "released"
    );
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn owner_identity_schema_and_corrupt_launch_evidence_never_authorize_reclaim() {
    let (seed, public, reconciler) = setup("pending_bad_evidence").await;
    let launches = seed
        .client
        .database(&seed.database)
        .collection::<Document>(LAUNCHES);
    let mut retained = Vec::new();
    for index in 0..4 {
        let lost = leak(&seed, &public, index).await;
        match index {
            0 => {
                admissions(&seed)
                    .update_one(
                        receipt_key(lost.id.as_str()),
                        doc! {"$set":{"owner":"another-owner"}},
                    )
                    .await
                    .unwrap();
            }
            1 => {
                admissions(&seed)
                    .update_one(
                        receipt_key(lost.id.as_str()),
                        doc! {"$set":{"document_schema_version":2_i64}},
                    )
                    .await
                    .unwrap();
            }
            2 => {
                launches
                    .update_one(
                        doc! {"_id":lost.id.as_str()},
                        doc! {"$set":{"record.id":"another-launch"}},
                    )
                    .await
                    .unwrap();
            }
            _ => {
                launches
                    .update_one(
                        doc! {"_id":lost.id.as_str()},
                        doc! {"$set":{"record":"corrupt"}},
                    )
                    .await
                    .unwrap();
            }
        }
        retained.push(lost);
    }
    assert_eq!(reconciler.sweep(None).await.unwrap().released, 0);
    assert_eq!(pending(&seed).await, 4);
    for lost in retained {
        assert_eq!(
            receipt(&seed, &lost).await.get_str("state").unwrap(),
            "reserved"
        );
    }
    seed.client.database(&seed.database).drop().await.unwrap();
}
