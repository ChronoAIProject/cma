use super::*;

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_baseline_resumes_after_persisted_progress_without_resetting_usage() {
    let database = database().await;
    let quota = MongoDocumentQuotaStore::new(&database, true);
    let id = uuid::Uuid::new_v4().to_string();
    quota.prepare(&id).await.unwrap();
    let mut sources: Vec<_> = (1..=40)
        .map(|size| source("owner", 1, &vec![b'x'; size]))
        .collect();
    sources.sort_by_key(DocumentSourceReservation::object_key);
    let row = object_row(&sources[35]).unwrap();
    database.run_command(doc! {"collMod": QUOTA_COLLECTION, "validator": {"_id": {"$ne": row.get_str("_id").unwrap()}}, "validationLevel": "strict"}).await.unwrap();
    let inventory = inventory(&id, &sources);
    assert_eq!(
        quota.install(&database, &inventory, true).await,
        Err(DocumentError::Unavailable)
    );
    let state = quota
        .rows
        .find_one(doc! {"_id": STATE})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(number(&state, "next_row").unwrap(), 32);
    assert!(!ready(&state));
    database
        .run_command(doc! {"collMod": QUOTA_COLLECTION, "validator": {}})
        .await
        .unwrap();
    quota.install(&database, &inventory, true).await.unwrap();
    assert_eq!(usage(&quota, "owner").await, [1, 820, 40]);
    quota.reserve(&sources[0]).await.unwrap();
    assert_eq!(usage(&quota, "owner").await, [1, 820, 40]);
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set with test failpoints"]
async fn document_quota_ambiguous_transaction_commit_reconciles_without_double_charge() {
    let database = database().await;
    let store = MongoDocumentQuotaStore::new(&database, true);
    activate(&store, &database, &[]).await;
    let admin = database.client().database("admin");
    admin.run_command(doc! {"configureFailPoint": "failCommand", "mode": {"times": 1}, "data": {
        "failCommands": ["commitTransaction"], "writeConcernError": {"code": 64, "errmsg": "quota fixture uncertain acknowledgement"},
        "errorLabels": ["UnknownTransactionCommitResult"]
    }}).await.unwrap();
    let source = source("owner", 1, b"body");
    let result = store.reserve(&source).await;
    admin
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    result.unwrap();
    store.reserve(&source).await.unwrap();
    assert_eq!(usage(&store, "owner").await, [1, 4, 1]);
    database.drop().await.unwrap();
}
