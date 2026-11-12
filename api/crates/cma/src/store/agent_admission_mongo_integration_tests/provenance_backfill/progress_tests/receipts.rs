use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn progress_requires_explicit_activation_and_retains_original_facts() {
    let (client, name, store, _, _, request) = legacy_fixture("receipt_private_agent").await;
    let db = client.database(&name);
    let owner = worker();
    let before = store
        .agent_provenance_snapshot(&request.agent_id)
        .await
        .unwrap();
    assert_eq!(
        store
            .durable_provenance_backfill_step(&owner)
            .await
            .unwrap(),
        None
    );
    assert_eq!(status(&db).await.unwrap()["phase"], "inactive");
    initialize(&db).await.unwrap();
    let ready = counts(&store).await;
    initialize(&db).await.unwrap();
    assert_eq!(counts(&store).await, ready);
    assert_eq!(
        store
            .durable_provenance_backfill_step(&owner)
            .await
            .unwrap(),
        Some(ProvenanceBackfillProgress::Advanced)
    );
    let progress = counts(&store).await;
    assert_eq!(
        progress.get_str("cursor").unwrap(),
        request.agent_id.as_str()
    );
    assert_eq!(progress.get_str("owner").unwrap(), owner);
    assert_eq!(
        progress.get_document("counts").unwrap(),
        &doc! {"scanned":1_i64,"recorded":1_i64,"retained":0_i64,"skipped":0_i64,"errors":0_i64}
    );
    let after = store
        .agent_provenance_snapshot(&request.agent_id)
        .await
        .unwrap();
    assert_eq!(before.resource, after.resource);
    assert_eq!(before.first_log, after.first_log);
    assert_eq!(
        after.creation.unwrap(),
        before.creation_provenance(&request.agent_id).unwrap()
    );
    assert_eq!(
        store
            .durable_provenance_backfill_step(&owner)
            .await
            .unwrap(),
        Some(ProvenanceBackfillProgress::SweepFinished)
    );
    let completed = counts(&store).await;
    let receipt_id = completed.get_str("last_receipt").unwrap();
    let receipt = db
        .collection::<Document>(COLLECTION)
        .find_one(doc! {"_id":receipt_id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.get_str("phase").unwrap(), "sweep_finished");
    let encoded = mongodb::bson::serialize_to_vec(&receipt).unwrap();
    for private in [request.agent_id.as_str(), owner.as_str()] {
        assert!(!encoded
            .windows(private.len())
            .any(|bytes| bytes == private.as_bytes()));
    }
    assert!(receipt.get("cursor").unwrap().as_null().is_some());
    assert!(receipt.get("owner").unwrap().as_null().is_some());
    assert_eq!(
        completed.get_str("cursor").unwrap(),
        request.agent_id.as_str()
    );
    assert_eq!(completed.get_str("owner").unwrap(), owner);
    assert_eq!(status(&db).await.unwrap()["global_completion"], false);
    assert_eq!(
        store
            .durable_provenance_backfill_step(&worker())
            .await
            .unwrap(),
        Some(ProvenanceBackfillProgress::Busy)
    );
    initialize(&db).await.unwrap();
    assert_eq!(
        db.collection::<Document>(COLLECTION)
            .find_one(doc! {"_id":receipt_id})
            .await
            .unwrap(),
        Some(receipt.clone())
    );
    assert_eq!(counts(&store).await, completed);
    let public = status(&db).await.unwrap().to_string();
    assert!(!public.contains(request.agent_id.as_str()));
    assert!(!public.contains(&owner));
    let collection = db.collection::<Document>(COLLECTION);
    collection
        .update_one(
            doc! {"_id":"agent-provenance-v1"},
            doc! {"$set":{"next_sweep_at":mongodb::bson::DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    for expected in [
        ProvenanceBackfillProgress::Advanced,
        ProvenanceBackfillProgress::SweepFinished,
    ] {
        assert_eq!(
            store
                .durable_provenance_backfill_step(&owner)
                .await
                .unwrap(),
            Some(expected)
        );
    }
    let next = counts(&store).await;
    assert_ne!(
        next.get_str("run").unwrap(),
        completed.get_str("run").unwrap()
    );
    assert_eq!(
        next.get_document("counts")
            .unwrap()
            .get_i64("retained")
            .unwrap(),
        1
    );
    assert_eq!(
        collection.find_one(doc! {"_id":receipt_id}).await.unwrap(),
        Some(receipt)
    );
    assert_eq!(status(&db).await.unwrap()["global_completion"], false);
    db.drop().await.unwrap();
}
