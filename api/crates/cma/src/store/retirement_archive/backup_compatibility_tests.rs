use super::super::schema::{
    PRE_BACKUP_ACCOUNT_COLLECTIONS, PRE_BACKUP_ACCOUNT_SHA256, PRE_BACKUP_COLLECTIONS,
    PRE_BACKUP_SHA256,
};
use super::*;

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn pre_backup_archive_readback_preserves_owned_backup_debt_in_current_census() {
    historical_archive_readback(PRE_BACKUP_SHA256, PRE_BACKUP_COLLECTIONS).await;
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn account_pre_backup_archive_readback_preserves_owned_backup_debt_in_current_census() {
    historical_archive_readback(PRE_BACKUP_ACCOUNT_SHA256, PRE_BACKUP_ACCOUNT_COLLECTIONS).await;
}

async fn historical_archive_readback(schema_sha256: &str, collections: &[&str]) {
    let (temp, store, request, bytes) = fixture().await;
    store.admit(&request, &bytes).await.unwrap();
    let custody = Custody::default();
    let current = export::build(&store, &request, &bytes, &custody, None)
        .await
        .unwrap();
    let mut previous = current.clone();
    previous.schema_sha256 = schema_sha256.into();
    previous
        .database
        .retain(|row| collections.contains(&row.collection.as_str()));
    let receipt = custody
        .retain(
            &request.operation_id,
            serde_json::to_vec(&previous).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        verify::load(&custody, &receipt, &request, &bytes)
            .await
            .unwrap(),
        previous
    );
    verify::restore(
        &custody,
        &receipt,
        &request,
        &bytes,
        &temp.0.join("previous-restore"),
    )
    .await
    .unwrap();
    assert!(super::super::matches_current(&previous, current).unwrap());
    let held = store.inspect(&request.sandbox_id).await.unwrap();
    for (collection, row, other) in [
        (
            crate::store::home_backups::SCHEDULES_COLLECTION,
            doc! {"_id":&request.sandbox_id,"document_schema_version":1_i64,"payload":"retained"},
            doc! {"_id":"other-home","document_schema_version":1_i64,"payload":"excluded"},
        ),
        (
            crate::store::home_backups::BACKUPS_COLLECTION,
            doc! {"_id":"owned-backup","sandbox_id":&request.sandbox_id,"document_schema_version":1_i64,"payload":"retained"},
            doc! {"_id":"other-backup","sandbox_id":"other-home","document_schema_version":1_i64,"payload":"excluded"},
        ),
    ] {
        store
            .database
            .collection::<Document>(collection)
            .insert_many([row, other])
            .await
            .unwrap();
    }
    let fresh = export::build(&store, &request, &bytes, &custody, None)
        .await
        .unwrap();
    for collection in [
        crate::store::home_backups::SCHEDULES_COLLECTION,
        crate::store::home_backups::BACKUPS_COLLECTION,
    ] {
        assert_eq!(
            fresh
                .database
                .iter()
                .find(|row| row.collection == collection)
                .unwrap()
                .documents,
            1
        );
    }
    assert!(!super::super::matches_current(&previous, fresh).unwrap());
    assert_eq!(store.inspect(&request.sandbox_id).await.unwrap(), held);
    assert_eq!(
        verify::load(&custody, &receipt, &request, &bytes)
            .await
            .unwrap(),
        previous
    );
    store.database.drop().await.unwrap();
}
