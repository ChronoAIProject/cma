use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn heartbeat_remap_and_deletion_each_refuse_stale_measured_publication() {
    for change in ["heartbeat", "mapping", "deletion"] {
        let f = Fixture::new().await;
        let mut binding = f.bind().await;
        let lease = f.lease().await;
        let publication = f.input(&lease).await.freeze(f.samples(60.0), true).unwrap();
        match change {
            "heartbeat" => {
                binding.revision += 1;
                f.store
                    .collection(sandbox_metric_bindings::COLLECTION)
                    .replace_one(doc! {"_id":&binding.id}, binding_document(&binding))
                    .await
                    .unwrap();
            }
            "mapping" => {
                f.store
                    .collection(usage_reports::CONTROL)
                    .update_one(doc! {"_id":"observer"}, doc! {"$inc":{"revision":1_i64}})
                    .await
                    .unwrap();
            }
            _ => {
                f.store
                    .collection(sandbox_metric_bindings::COLLECTION)
                    .delete_one(doc! {"_id":&binding.id})
                    .await
                    .unwrap();
            }
        }
        assert!(
            matches!(
                f.store.publish_measured(&lease, &publication).await,
                Err(SandboxStoreError::Conflict)
            ),
            "{change}"
        );
        assert!(f.rows(EVIDENCE).await.is_empty());
        assert!(f.rows(OUTBOX).await.is_empty());
        let row = f
            .store
            .collection(INVENTORY)
            .find_one(doc! {"_id":keys::id(&lease.stream.key())})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(MeasuredStream::read(&row).unwrap(), lease.stream);
        f.db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn expired_worker_and_duplicate_event_failure_leave_no_partial_fact() {
    let f = Fixture::new().await;
    f.bind().await;
    let old = f.lease().await;
    let publication = f.input(&old).await.freeze(f.samples(60.0), true).unwrap();
    f.store
        .collection(INVENTORY)
        .update_one(
            doc! {"_id":keys::id(&old.stream.key())},
            doc! {"$set":{"lease_until":mongodb::bson::DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    let current = f.lease().await;
    assert!(current.generation > old.generation);
    assert!(f.store.publish_measured(&old, &publication).await.is_err());
    f.store.collection(OUTBOX).insert_one(doc! {"_id":"synthetic-sequence-collision","stream_id":&current.stream.id,"sequence":2_i64,"source_id":"synthetic-unrelated","part":0_i64}).await.unwrap();
    let publication = f
        .input(&current)
        .await
        .freeze(f.samples(60.0), true)
        .unwrap();
    assert!(f
        .store
        .publish_measured(&current, &publication)
        .await
        .is_err());
    assert!(f.rows(EVIDENCE).await.is_empty());
    assert_eq!(f.rows(OUTBOX).await.len(), 1);
    let row = f
        .store
        .collection(INVENTORY)
        .find_one(doc! {"_id":keys::id(&current.stream.key())})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(MeasuredStream::read(&row).unwrap(), current.stream);
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn committed_binding_write_fence_blocks_concurrent_deletion_until_proof_is_retained() {
    let f = Fixture::new().await;
    let binding = f.bind().await;
    let lease = f.lease().await;
    let publication = f.input(&lease).await.freeze(f.samples(60.0), true).unwrap();
    let mut session = f.store.client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    assert!(matches!(
        f.store
            .publish_measured_in(&mut session, &lease, &publication)
            .await,
        Ok(true)
    ));
    let collection = f.store.collection(sandbox_metric_bindings::COLLECTION);
    let (started, seen) = tokio::sync::oneshot::channel();
    let mut deletion = tokio::spawn(async move {
        started.send(()).unwrap();
        collection
            .delete_one(doc! {"_id":binding.id})
            .await
            .unwrap()
    });
    seen.await.unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(150), &mut deletion)
            .await
            .is_err(),
        "deletion must conflict with the exact source-row write"
    );
    assert!(matches!(
        crate::store::sandbox_mongo::commit_transaction(&mut session).await,
        crate::store::sandbox_mongo::CommitOutcome::Committed
    ));
    assert_eq!(deletion.await.unwrap().deleted_count, 1);
    assert!(f.rows(sandbox_metric_bindings::COLLECTION).await.is_empty());
    let evidence = f.rows(EVIDENCE).await;
    assert_eq!(evidence.len(), 1);
    let proof: super::super::freeze::Manifest =
        serde_json::from_slice(evidence[0].get_binary_generic("manifest").unwrap()).unwrap();
    proof.frozen.unwrap().verify_replay().unwrap();
    assert_eq!(f.events().await.len(), 2);
    f.db.drop().await.unwrap();
}
