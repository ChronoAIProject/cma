use super::*;

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn measured_custody_schema_is_explicit_and_resumes_existing_index() {
    let (client, database, _) = fixture().await;
    let step = Step::MeasuredCustody;
    let ledger = ledger::Ledger::for_step(client, database.clone(), step);
    let pending = step.inspect(&database).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(database.list_collection_names().await.unwrap().is_empty());

    let (collection, index) = pending.into_iter().next().unwrap();
    ledger.initialize().await.unwrap();
    ledger.claim(&identity()).await.unwrap();
    assert!(step.create(&database, collection, index).await.unwrap());
    expire(&database).await;
    assert_eq!(
        apply_indexes(&ledger, &database, &identity())
            .await
            .unwrap()["outcome"],
        "completed"
    );
    assert_eq!(
        apply_indexes(&ledger, &database, &identity())
            .await
            .unwrap()["outcome"],
        "already_completed"
    );
    let rows = database.collection::<Document>(collection);
    assert_eq!(rows.count_documents(doc! {}).await.unwrap(), 0);
    database
        .run_command(doc! {
            "createIndexes": collection,
            "indexes": [{"name": "unexpected_retention", "key": {"expires_at": 1},
                "expireAfterSeconds": 60}],
        })
        .await
        .unwrap();
    assert!(step.inspect(&database).await.is_err());
    database.drop().await.unwrap();
}
