use super::*;

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn conditional_queries_inspect_without_writes_or_private_sample_output() {
    let (client, database) = fixture().await;
    for collection in [
        "sandbox_usage_events",
        "sandbox_usage_reports",
        "sandboxes",
        "workspaces",
        "native_prepared_sandboxes",
    ] {
        database.create_collection(collection).await.unwrap();
    }
    database.collection::<Document>("sandbox_usage_events").insert_one(doc! {
        "sandbox_id":"private-sandbox-canary","started_at":90001_i64,"ended_at":90002_i64,"revision":1
    }).await.unwrap();
    database
        .collection::<Document>("sandbox_usage_reports")
        .insert_one(doc! {
            "reserved":true,"logical_expires_at":1_i64,"private":"payload-canary"
        })
        .await
        .unwrap();
    for query in catalog::all().into_iter().filter(|q| q.step.is_none()) {
        let output = inspection::inspect_query(&database, &query).await.unwrap();
        assert_eq!(output["inspection_only"], true);
        assert_eq!(output["index_present"], false);
        assert!(!output.to_string().contains("private-sandbox-canary"));
        assert!(!output.to_string().contains("payload-canary"));
        assert!(
            build::execute(&query, client.clone(), database.clone(), &identity(), 60)
                .await
                .is_err()
        );
        assert!(abort::execute(
            &query,
            client.clone(),
            database.clone(),
            &identity(),
            &image(),
            1
        )
        .await
        .is_err());
        let inventory = database
            .run_command(doc! {"listIndexes":query.collection})
            .await
            .unwrap();
        assert_eq!(
            super::super::super::index_inventory::entries(&inventory)
                .unwrap()
                .len(),
            1
        );
    }
    // A real matching partial candidate is inspectable but remains without a manifest.
    let partial = catalog::select("usage-reserved-expiry").unwrap().remove(0);
    database
        .run_command(doc! {"createIndexes":partial.collection,"indexes":[partial.index.clone()]})
        .await
        .unwrap();
    assert_eq!(
        inspection::inspect_query(&database, &partial)
            .await
            .unwrap()["index_present"],
        true
    );
    assert!(partial.migration().is_err());
    assert_eq!(database.list_collection_names().await.unwrap().len(), 6);
    database.drop().await.unwrap();
}
