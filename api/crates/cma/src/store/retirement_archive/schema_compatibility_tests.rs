use super::super::schema::{
    LEGACY_COLLECTIONS, LEGACY_SHA256, PRE_LEGAL_CONSENT_COLLECTIONS, PRE_LEGAL_CONSENT_SHA256,
};
use super::*;

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn legacy_archive_readback_survives_upgrade_without_omitting_new_owned_data() {
    let (temp, store, request, bytes) = fixture().await;
    store.admit(&request, &bytes).await.unwrap();
    let custody = Custody::default();
    let current = export::build(&store, &request, &bytes, &custody, None)
        .await
        .unwrap();
    let audit: Vec<_> = super::super::census_catalog::AUDIT
        .iter()
        .copied()
        .filter(|(name, _)| LEGACY_COLLECTIONS.contains(name))
        .collect();
    let names: Vec<_> = crate::store::retirement_archive::manifest_collections()
        .iter()
        .copied()
        .filter(|name| LEGACY_COLLECTIONS.contains(name))
        .collect();
    let mut old = current.clone();
    old.schema_sha256 =
        digest(format!("archive-census-v2\n{audit:?}\n{}", names.join("\n")).as_bytes());
    assert_eq!(old.schema_sha256, LEGACY_SHA256);
    old.database
        .retain(|r| LEGACY_COLLECTIONS.contains(&r.collection.as_str()));
    let receipt = custody
        .retain(&request.operation_id, serde_json::to_vec(&old).unwrap())
        .await
        .unwrap();
    assert_eq!(
        verify::load(&custody, &receipt, &request, &bytes)
            .await
            .unwrap(),
        old
    );
    let restored = verify::restore(
        &custody,
        &receipt,
        &request,
        &bytes,
        &temp.0.join("old-restore"),
    )
    .await
    .unwrap();
    assert_eq!(
        restored.documents,
        old.database.iter().map(|r| r.documents).sum::<u64>()
    );
    assert!(super::super::matches_current(&old, current).unwrap());
    let held = store.inspect(&request.sandbox_id).await.unwrap();
    let collection = store
        .database
        .collection::<Document>(crate::store::allocation_buckets::COLLECTION);
    collection.insert_many([
        doc! {"_id":"owned-day","document_schema_version":1_i64,"sandbox_id":&request.sandbox_id,"payload":"retained"},
        doc! {"_id":"other-day","document_schema_version":1_i64,"sandbox_id":"sbx_other","payload":"excluded"},
    ]).await.unwrap();
    let fresh = export::build(&store, &request, &bytes, &custody, None)
        .await
        .unwrap();
    let row = fresh
        .database
        .iter()
        .find(|r| r.collection == crate::store::allocation_buckets::COLLECTION)
        .unwrap();
    assert_eq!(row.documents, 1);
    assert!(!super::super::matches_current(&old, fresh.clone()).unwrap());
    assert_eq!(store.inspect(&request.sandbox_id).await.unwrap(), held);
    let fresh_receipt = custody
        .retain(&request.operation_id, serde_json::to_vec(&fresh).unwrap())
        .await
        .unwrap();
    verify::restore(
        &custody,
        &fresh_receipt,
        &request,
        &bytes,
        &temp.0.join("new-restore"),
    )
    .await
    .unwrap();
    let retained = std::fs::read(
        temp.0
            .join("new-restore/database/sandbox_usage_allocation_buckets_v1.bson"),
    )
    .unwrap();
    let retained: Document = mongodb::bson::deserialize_from_slice(&retained).unwrap();
    assert_eq!(retained.get_str("_id").unwrap(), "owned-day");
    // The legacy manifest remains valid offline history, never authority to omit
    // the new row in a live completion census.
    verify::load(&custody, &receipt, &request, &bytes)
        .await
        .unwrap();
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn populated_release_lanes_preserve_pre_pointer_archive_readback() {
    let (_temp, store, request, bytes) = fixture().await;
    store.admit(&request, &bytes).await.unwrap();
    let custody = Custody::default();
    let predecessor = export::build(&store, &request, &bytes, &custody, None)
        .await
        .unwrap();
    let release_collections = [
        crate::store::released_runtimes::COLLECTION,
        crate::store::released_runtimes::TRANSITIONS_COLLECTION,
    ];
    for name in release_collections {
        store
            .database
            .collection::<Document>(name)
            .insert_one(doc! {"_id":"deployment-release"})
            .await
            .unwrap();
    }
    let written = export::publish(&store, &request, &bytes, &custody, None)
        .await
        .unwrap();
    let current = verify::load(&custody, &written, &request, &bytes)
        .await
        .unwrap();
    // These are the preceding reader's exact schema and namespace checks.
    assert_eq!(current.schema_sha256, PRE_LEGAL_CONSENT_SHA256);
    assert_eq!(
        current
            .database
            .iter()
            .map(|row| row.collection.as_str())
            .collect::<Vec<_>>(),
        PRE_LEGAL_CONSENT_COLLECTIONS,
    );
    assert!(super::super::matches_current(&predecessor, current).unwrap());
    for name in release_collections {
        assert_eq!(
            store
                .database
                .collection::<Document>(name)
                .count_documents(doc! {})
                .await
                .unwrap(),
            1,
        );
    }
    store.database.drop().await.unwrap();
}
