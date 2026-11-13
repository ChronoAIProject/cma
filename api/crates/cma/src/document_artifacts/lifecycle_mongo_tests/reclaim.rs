use super::*;

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_reclaim_handoff_does_not_starve_ordinary_reservations() {
    let (db, store, service) = fixture().await;
    let document = created(&service).await;
    let source = ledger(&db, &store.get(&document.id).await.unwrap().unwrap()).await;
    let rows = db.collection::<Document>(QUOTA_COLLECTION);
    rows.update_one(
        doc! {"kind": "object"},
        doc! {"$set": {"state": "condemned", "reserved_at": DateTime::from_millis(1)}},
    )
    .await
    .unwrap();
    for n in 0..63 {
        let mut row = super::super::quota::object_row(&DocumentSourceReservation {
            sha256: digest(format!("owned-by-purge-{n}").as_bytes()),
            bytes: 1,
            ..source.clone()
        })
        .unwrap();
        row.insert("state", "condemned");
        row.insert("reserved_at", DateTime::from_millis(1));
        rows.insert_one(row).await.unwrap();
    }
    let ordinary = DocumentSourceReservation {
        document_id: format!("doc_{:032x}", 99),
        sha256: digest(b"ordinary orphan"),
        bytes: 7,
        ..source.clone()
    };
    let mut row = super::super::quota::object_row(&ordinary).unwrap();
    row.insert("reserved_at", DateTime::from_millis(1));
    rows.insert_one(row).await.unwrap();
    rows.insert_one(super::super::quota::document_row(&ordinary))
        .await
        .unwrap();
    rows.update_one(
        doc! {"kind": "owner"},
        doc! {"$inc": {"documents": 1_i64, "objects": 64_i64, "bytes": 70_i64}},
    )
    .await
    .unwrap();
    rows.insert_one(doc! {
        "_id":"activation","kind":"activation","document_schema_version":1_i64,
        "phase":"ready","inventory_sha256":digest(b"inventory"),
        "activation_id":uuid::Uuid::new_v4().to_string(),"baseline_rows":0_i64,
        "next_row":0_i64,"activated_at":DateTime::from_millis(1)
    })
    .await
    .unwrap();
    deleted(&service, &document.id, 1).await;
    // No purge worker runs: unsupported or paused purge must not block reclaim
    // from reaching independent reservations behind its permanently handed-off rows.
    let reclaim = DocumentMaintenance::new(
        Arc::new(store.reclaim(true)),
        Arc::new(memory::MemoryDocumentBlobs::default()),
        true,
    );
    let mut released = 0;
    for _ in 0..3 {
        released += reclaim
            .reclaim_step(DateTime::now().timestamp_millis())
            .await
            .unwrap()
            .released;
    }
    assert_eq!(
        released, 1,
        "a full condemned batch owned by purge must not starve ordinary reservations"
    );
    let owner = rows
        .find_one(doc! {"kind": "owner"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(owner.get_i64("documents").unwrap(), 1);
    assert_eq!(owner.get_i64("objects").unwrap(), 64);
    assert_eq!(owner.get_i64("bytes").unwrap(), source.bytes as i64 + 63);
    assert_eq!(
        rows.count_documents(doc! {"kind":"object","state":"condemned"})
            .await
            .unwrap(),
        64
    );
    assert!(rows
        .find_one(doc! {"_id": format!("document:{}:{}",ordinary.owner_hash,ordinary.document_id)})
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store.purge_state(&document.id).await,
        Ok(Some(DocumentPurgeState::Pending))
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_reclaim_owned_lookup_preserves_malformed_id_refusal() {
    use mongodb::bson::Bson;
    for shape in [
        "prefix",
        "non_string",
        "extra_component",
        "trailing_newline",
    ] {
        let (db, store, service) = fixture().await;
        let document = created(&service).await;
        let source = ledger(&db, &store.get(&document.id).await.unwrap().unwrap()).await;
        let id = match shape {
            "prefix" => Bson::String(format!(
                "bad:{}:{}:{}",
                source.owner_hash, source.document_id, source.sha256
            )),
            "non_string" => Bson::Int64(7),
            "trailing_newline" => Bson::String(format!(
                "object:{}:{}:{}\n",
                source.owner_hash, source.document_id, source.sha256
            )),
            _ => Bson::String(format!(
                "object:{}:{}:{}:extra",
                source.owner_hash, source.document_id, source.sha256
            )),
        };
        let rows = db.collection::<Document>(QUOTA_COLLECTION);
        let mut malformed = super::super::quota::object_row(&source).unwrap();
        malformed.insert("_id", id.clone());
        malformed.insert("state", "condemned");
        malformed.insert("reserved_at", DateTime::from_millis(1));
        rows.insert_one(malformed.clone()).await.unwrap();
        deleted(&service, &document.id, 1).await;
        assert_eq!(
            store
                .reclaim(true)
                .candidates(DateTime::now().timestamp_millis(), 64)
                .await
                .err(),
            Some(DocumentError::Corrupt),
            "purge ownership must not hide malformed object IDs: {shape}"
        );
        assert_eq!(
            rows.find_one(doc! {"_id": id}).await.unwrap(),
            Some(malformed)
        );
        db.drop().await.unwrap();
    }
}
