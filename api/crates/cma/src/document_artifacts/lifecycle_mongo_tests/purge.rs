use super::*;

async fn claimed() -> (
    Database,
    Arc<MongoDocumentStore>,
    PurgeLease,
    DocumentSourceReservation,
) {
    let (db, store, service) = fixture().await;
    let document = created(&service).await;
    let source = ledger(&db, &store.get(&document.id).await.unwrap().unwrap()).await;
    deleted(&service, &document.id, 1).await;
    let purge = store.purge();
    let lease = purge
        .claim(&document.id, &uuid::Uuid::new_v4().to_string(), 0)
        .await
        .unwrap()
        .unwrap();
    (db, store, lease, source)
}
#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_purge_release_is_exactly_once_across_two_stores() {
    let (db, store, lease, source) = claimed().await;
    let a = store.purge();
    let b = MongoDocumentStore::new(db.clone()).purge();
    let (a, b) = tokio::join!(a.release(&lease), b.release(&lease));
    assert_eq!(usize::from(a == Ok(true)) + usize::from(b == Ok(true)), 1);
    let owner = db
        .collection::<Document>(QUOTA_COLLECTION)
        .find_one(doc! {"_id":format!("owner:{}",source.owner_hash)})
        .await
        .unwrap()
        .unwrap();
    for key in ["documents", "bytes", "objects"] {
        assert_eq!(owner.get_i64(key).unwrap(), 0);
    }
    assert_eq!(
        store.purge_state(&lease.document.id).await,
        Ok(Some(DocumentPurgeState::Complete))
    );
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_expired_purge_holder_cannot_advance_or_release() {
    let (db, store, lease, _) = claimed().await;
    let lifecycle = db.collection::<Document>(LIFECYCLE_COLLECTION);
    lifecycle
        .update_one(
            doc! {"_id":&lease.document.id},
            doc! {"$set":{"purge.lease.expires_at":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    let purge = store.purge();
    assert_eq!(
        purge.advance(&lease, PurgeStep::Retry { next_at: 0 }).await,
        Ok(false)
    );
    assert_eq!(
        purge.block(&lease, "storage_deletion_unsupported").await,
        Ok(false)
    );
    assert_eq!(purge.release(&lease).await, Ok(false));
    let newer = purge
        .claim(
            &lease.document.id,
            &uuid::Uuid::new_v4().to_string(),
            i64::MAX,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(newer.fence > lease.fence);
    assert_eq!(purge.release(&lease).await, Ok(false));
    assert_eq!(purge.release(&newer).await, Ok(true));
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_claim_uses_server_time_and_cannot_steal_live_lease() {
    let (db, store, lease, _) = claimed().await;
    assert!(lease.expires_at > DateTime::now().timestamp_millis());
    assert_eq!(
        store
            .purge()
            .claim(
                &lease.document.id,
                &uuid::Uuid::new_v4().to_string(),
                i64::MAX
            )
            .await,
        Ok(None)
    );
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_reclaim_never_claims_or_finalizes_deleted_document_objects() {
    let (db, store, service) = fixture().await;
    let document = created(&service).await;
    let source = ledger(&db, &store.get(&document.id).await.unwrap().unwrap()).await;
    let rows = db.collection::<Document>(QUOTA_COLLECTION);
    rows.update_one(
        doc! {"kind":"object"},
        doc! {"$set":{"reserved_at":DateTime::from_millis(1)}},
    )
    .await
    .unwrap();
    let reclaim = store.reclaim(true);
    let candidates = reclaim
        .candidates(DateTime::now().timestamp_millis(), 64)
        .await
        .unwrap();
    let candidate = candidates.into_iter().find(|c| c.source == source).unwrap();
    deleted(&service, &document.id, 1).await;
    let lease = store
        .purge()
        .claim(&document.id, &uuid::Uuid::new_v4().to_string(), 0)
        .await
        .unwrap()
        .unwrap();
    assert!(reclaim
        .candidates(DateTime::now().timestamp_millis(), 64)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(reclaim.judge(&candidate).await, Ok(Judgement::OwnedByPurge));
    rows.update_one(doc! {"kind":"object"}, doc! {"$set":{"state":"condemned"}})
        .await
        .unwrap();
    assert!(reclaim
        .claim_delete(
            &candidate,
            &uuid::Uuid::new_v4().to_string(),
            DateTime::now().timestamp_millis()
        )
        .await
        .unwrap()
        .is_none());
    assert!(store.purge().release(&lease).await.unwrap());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_purge_without_ledger_completes_without_creating_quota_rows() {
    let (db, store, service) = fixture().await;
    let document = created(&service).await;
    deleted(&service, &document.id, 1).await;
    let purge = store.purge();
    let lease = purge
        .claim(&document.id, &uuid::Uuid::new_v4().to_string(), 0)
        .await
        .unwrap()
        .unwrap();
    assert!(purge.release(&lease).await.unwrap());
    assert_eq!(
        db.collection::<Document>(QUOTA_COLLECTION)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_purge_progress_resumes_bounded_keys_and_preserves_latest_reservation_time(
) {
    let (db, store, service) = fixture().await;
    let document = created(&service).await;
    let source = ledger(&db, &store.get(&document.id).await.unwrap().unwrap()).await;
    let rows = db.collection::<Document>(QUOTA_COLLECTION);
    let reserved = DateTime::now();
    for n in 0..12 {
        let mut row = super::super::quota::object_row(&DocumentSourceReservation {
            sha256: digest(format!("object-{n}").as_bytes()),
            ..source.clone()
        })
        .unwrap();
        row.insert("reserved_at", reserved);
        rows.insert_one(row).await.unwrap();
    }
    let purge = store.purge();
    deleted(&service, &document.id, 1).await;
    let lease = purge
        .claim(&document.id, &uuid::Uuid::new_v4().to_string(), 0)
        .await
        .unwrap()
        .unwrap();
    let first = purge.objects(&lease).await.unwrap();
    assert_eq!(first.len(), PURGE_KNOWN_BATCH + 1);
    assert!(purge
        .advance(
            &lease,
            PurgeStep::Progress {
                position: PurgePosition::KnownObjects {
                    after: first[3].clone()
                },
                next_at: 0
            }
        )
        .await
        .unwrap());
    let resumed = purge
        .claim(&lease.document.id, &uuid::Uuid::new_v4().to_string(), 0)
        .await
        .unwrap()
        .unwrap();
    assert!(resumed.document.last_reserved_at.unwrap() >= reserved.timestamp_millis());
    let next = purge.objects(&resumed).await.unwrap();
    assert_eq!(next.len(), PURGE_KNOWN_BATCH + 1);
    assert!(next.iter().all(|key| key > &first[3]));
    assert_eq!(next[0], first[4]);
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_purge_counter_underflow_rolls_back_every_row() {
    let (db, store, lease, _) = claimed().await;
    let rows = db.collection::<Document>(QUOTA_COLLECTION);
    rows.update_one(doc! {"kind":"owner"}, doc! {"$set":{"bytes":0_i64}})
        .await
        .unwrap();
    assert_eq!(
        store.purge().release(&lease).await,
        Err(DocumentError::Corrupt)
    );
    assert_eq!(
        rows.count_documents(doc! {"kind":"object"}).await.unwrap(),
        1
    );
    assert_eq!(
        rows.count_documents(doc! {"kind":"document"})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store.purge_state(&lease.document.id).await,
        Ok(Some(DocumentPurgeState::Pending))
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_reclaim_claim_from_before_delete_cannot_finalize_after_delete() {
    let (db, store, service) = fixture().await;
    let document = created(&service).await;
    let source = ledger(&db, &store.get(&document.id).await.unwrap().unwrap()).await;
    let rows = db.collection::<Document>(QUOTA_COLLECTION);
    rows.update_one(
        doc! {"kind":"object"},
        doc! {"$set":{"state":"condemned","reserved_at":DateTime::from_millis(1)}},
    )
    .await
    .unwrap();
    let reclaim = store.reclaim(true);
    let candidate = reclaim
        .candidates(DateTime::now().timestamp_millis(), 64)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let claim = reclaim
        .claim_delete(
            &candidate,
            &uuid::Uuid::new_v4().to_string(),
            DateTime::now().timestamp_millis(),
        )
        .await
        .unwrap()
        .unwrap();
    deleted(&service, &document.id, 1).await;
    assert_eq!(reclaim.finalize(&claim).await, Ok(false));
    assert_eq!(
        rows.count_documents(doc! {"kind":"object"}).await.unwrap(),
        1
    );
    let owner = rows
        .find_one(doc! {"_id":format!("owner:{}",source.owner_hash)})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(owner.get_i64("documents").unwrap(), 1);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_purge_releases_actual_rows_and_preserves_other_document_accounting() {
    let (db, store, lease, source) = claimed().await;
    let rows = db.collection::<Document>(QUOTA_COLLECTION);
    let orphan = DocumentSourceReservation {
        sha256: digest(b"orphan"),
        bytes: 6,
        ..source.clone()
    };
    let other = DocumentSourceReservation {
        document_id: format!("doc_{:032x}", 2),
        sha256: digest(b"elsewhere"),
        bytes: 9,
        ..source.clone()
    };
    let mut orphan_row = super::super::quota::object_row(&orphan).unwrap();
    orphan_row.insert("state", "condemned");
    rows.insert_one(orphan_row).await.unwrap();
    rows.insert_one(super::super::quota::object_row(&other).unwrap())
        .await
        .unwrap();
    rows.insert_one(super::super::quota::document_row(&other))
        .await
        .unwrap();
    rows.update_one(
        doc! {"kind":"owner"},
        doc! {"$inc":{"documents":1_i64,"objects":2_i64,"bytes":15_i64}},
    )
    .await
    .unwrap();
    assert_eq!(store.purge().release(&lease).await, Ok(true));
    let owner = rows.find_one(doc! {"kind":"owner"}).await.unwrap().unwrap();
    assert_eq!(owner.get_i64("documents").unwrap(), 1);
    assert_eq!(owner.get_i64("bytes").unwrap(), 9);
    assert_eq!(owner.get_i64("objects").unwrap(), 1);
    assert_eq!(
        rows.count_documents(doc! {"kind":"object"}).await.unwrap(),
        1
    );
    assert_eq!(
        rows.count_documents(doc! {"kind":"document"})
            .await
            .unwrap(),
        1
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_purge_rejects_malformed_object_bytes_and_rolls_back_every_row() {
    for invalid in [mongodb::bson::bson!("8"), mongodb::bson::bson!([8_i64])] {
        let (db, store, lease, source) = claimed().await;
        let rows = db.collection::<Document>(QUOTA_COLLECTION);
        // A valid numeric row keeps $sum numeric while a malformed row would be
        // silently ignored by MongoDB. The complete reservation must survive.
        let other = DocumentSourceReservation {
            sha256: digest(b"valid orphan"),
            bytes: 12,
            ..source.clone()
        };
        rows.insert_one(super::super::quota::object_row(&other).unwrap())
            .await
            .unwrap();
        rows.update_one(
            doc! {"kind": "owner"},
            doc! {"$inc": {"bytes": 12_i64, "objects": 1_i64}},
        )
        .await
        .unwrap();
        let original = super::super::quota::object_row(&source).unwrap();
        rows.update_one(
            doc! {"_id": original.get_str("_id").unwrap()},
            doc! {"$set": {"bytes": invalid}},
        )
        .await
        .unwrap();
        let before = rows
            .find_one(doc! {"kind": "owner"})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            store.purge().release(&lease).await,
            Err(DocumentError::Corrupt)
        );
        assert_eq!(
            rows.count_documents(doc! {"kind": "object"}).await.unwrap(),
            2
        );
        assert_eq!(
            rows.count_documents(doc! {"kind": "document"})
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            rows.find_one(doc! {"kind": "owner"}).await.unwrap(),
            Some(before),
            "a rejected release must roll back the owner fence and counters"
        );
        assert_eq!(
            store.purge_state(&lease.document.id).await,
            Ok(Some(DocumentPurgeState::Pending))
        );
        db.drop().await.unwrap();
    }
}
