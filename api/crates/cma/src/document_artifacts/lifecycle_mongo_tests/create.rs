use super::*;

pub(super) async fn record() -> StoredDocument {
    let rows = Arc::new(memory::MemoryDocuments::default());
    let service = DocumentService::new(
        rows.clone(),
        Arc::new(memory::MemoryDocumentBlobs::default()),
    );
    let document = created(&service).await;
    rows.get(&document.id).await.unwrap().unwrap()
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_ledgered_create_publishes_identity_metadata_and_source_atomically() {
    let (db, store, _) = fixture().await;
    let record = record().await;
    let source = ledger(&db, &record).await;
    assert_eq!(
        store
            .insert(&record, Some((&source, SourceTicket::Ledgered)))
            .await,
        Ok(InsertOutcome::Inserted)
    );
    let lifecycle = db
        .collection::<Document>(LIFECYCLE_COLLECTION)
        .find_one(doc! {"_id": &record.metadata.id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lifecycle.get_str("state").unwrap(), "live");
    assert_eq!(lifecycle.get_str("owner_hash").unwrap(), source.owner_hash);
    assert!(store.get(&record.metadata.id).await.unwrap().is_some());
    let object = db
        .collection::<Document>(QUOTA_COLLECTION)
        .find_one(doc! {"kind": "object"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(object.get_str("state").unwrap(), "committed");
    assert_eq!(
        store
            .insert(&record, Some((&source, SourceTicket::Ledgered)))
            .await,
        Ok(InsertOutcome::Exists)
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_ledgered_create_rolls_back_identity_and_metadata_for_condemned_source()
{
    let (db, store, _) = fixture().await;
    let record = record().await;
    let source = ledger(&db, &record).await;
    let rows = db.collection::<Document>(QUOTA_COLLECTION);
    rows.update_one(
        doc! {"kind": "object"},
        doc! {"$set": {"state": "condemned"}},
    )
    .await
    .unwrap();
    assert_eq!(
        store
            .insert(&record, Some((&source, SourceTicket::Ledgered)))
            .await,
        Err(DocumentError::Unavailable)
    );
    assert!(store.get(&record.metadata.id).await.unwrap().is_none());
    assert!(db
        .collection::<Document>(LIFECYCLE_COLLECTION)
        .find_one(doc! {"_id": &record.metadata.id})
        .await
        .unwrap()
        .is_none());
    let object = rows
        .find_one(doc! {"kind": "object"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(object.get_str("state").unwrap(), "condemned");
    let owner = rows
        .find_one(doc! {"kind": "owner"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(owner.get_i64("documents").unwrap(), 1);
    assert_eq!(owner.get_i64("objects").unwrap(), 1);
    assert_eq!(owner.get_i64("bytes").unwrap(), source.bytes as i64);
    db.drop().await.unwrap();
}
