use super::*;
use mongodb::{
    options::{ClientOptions, Credential, IndexOptions},
    Client,
};

#[test]
fn runtime_history_indexes_are_permanent() {
    let index = IndexModel::builder().keys(doc! {"_id":1}).build();
    assert!(indexes_valid(std::slice::from_ref(&index)));
    for options in [
        IndexOptions::builder()
            .expire_after(Duration::from_secs(1))
            .build(),
        IndexOptions::builder()
            .partial_filter_expression(doc! {"_id":"x"})
            .build(),
        IndexOptions::builder().sparse(true).build(),
        IndexOptions::builder().hidden(true).build(),
    ] {
        let mut invalid = index.clone();
        invalid.options = Some(options);
        assert!(!indexes_valid(&[invalid]));
    }
    assert!(!indexes_valid(&[]));
    assert!(!indexes_valid(&[index.clone(), index]));
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn worker_history_cross_client_visibility_append_only_grants_and_cutover_barrier() {
    let uri = std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_worker_keys_{}", uuid::Uuid::new_v4().simple());
    let db = admin.database(&name);
    let result = exercise(&uri, &db).await;
    db.drop().await.unwrap();
    result.unwrap();
}

async fn exercise(uri: &str, db: &Database) -> Result<(), Box<dyn std::error::Error>> {
    let name = db.name();
    let hello = db.run_command(doc! {"hello":1}).await?;
    assert!(
        hello.get_str("setName").is_ok(),
        "replica-set fixture required"
    );
    crate::records::mongo::migrate(db).await?;
    migrate(db).await?;
    migrate(db).await?;
    db.run_command(crate::records::mongo::role(name, false))
        .await?;
    db.run_command(role(name, false)).await?;
    let password = uuid::Uuid::new_v4().to_string();
    db.run_command(doc! {"createUser":"runtime","pwd":&password,"roles":[
        {"role":"cma_eg_runtime","db":name},
        {"role":"cma_eg_worker_credentials_runtime","db":name}
    ]})
    .await?;
    let connect = || {
        let password = password.clone();
        async move {
            let mut options = ClientOptions::parse(uri).await?;
            options.credential = Some(
                Credential::builder()
                    .username("runtime".to_owned())
                    .password(password)
                    .source(name.to_owned())
                    .build(),
            );
            Client::with_options(options)
        }
    };
    let writer_client = connect().await?;
    let reader_client = connect().await?;
    let writer_db = writer_client.database(name);
    let reader_db = reader_client.database(name);
    crate::records::mongo::verify(&writer_db).await?;
    refuse_untracked_after_cutover(&writer_db).await?;
    let writer = MongoWorkerHistory::open(writer_db.clone()).await?;
    let reader = MongoWorkerHistory::open(reader_db.clone()).await?;
    let digest = "a".repeat(64);
    assert!(!reader.was_worker(&digest).await?);
    writer.record(&digest).await?;
    assert!(reader.was_worker(&digest).await?);
    writer.record(&format!("sha256:{digest}")).await?;
    assert_eq!(
        db.collection::<Document>(USES)
            .count_documents(doc! {})
            .await?,
        1
    );
    let restarted = MongoWorkerHistory::open(connect().await?.database(name)).await?;
    assert!(restarted.was_worker(&digest).await?);
    assert!(writer_db
        .collection::<Document>(USES)
        .update_one(doc! {"_id":&digest}, doc! {"$set":{"schema_version":1}})
        .await
        .is_err());
    assert!(writer_db
        .collection::<Document>(USES)
        .delete_one(doc! {"_id":&digest})
        .await
        .is_err());
    assert!(writer_db
        .run_command(doc! {"collMod":USES,"validationLevel":"off"})
        .await
        .is_err());
    assert!(writer_db
        .collection::<Cutover>(CUTOVERS)
        .insert_one(super::super::tests::cutoff())
        .await
        .is_err());
    assert!(writer_db
        .collection::<Document>(MIGRATIONS)
        .delete_one(doc! {"_id":CUTOVER_BARRIER})
        .await
        .is_err());
    let cutoff = super::super::tests::cutoff();
    db.collection_with_options::<Cutover>(CUTOVERS, options())
        .insert_one(&cutoff)
        .await?;
    db.collection_with_options::<Document>(MIGRATIONS, options())
        .insert_one(doc! {"_id":CUTOVER_BARRIER,"version":1})
        .await?;
    assert_eq!(reader.cutover().await?, Some(cutoff));
    assert!(refuse_untracked_after_cutover(&reader_db).await.is_err());
    // Additive reruns preserve every independently installed migration marker.
    crate::records::mongo::migrate_thread_fences(db).await?;
    crate::records::mongo::migrate_whatsapp_flows(db).await?;
    crate::records::mongo::migrate_media_continuations(db).await?;
    crate::records::mongo::migrate(db).await?;
    migrate(db).await?;
    for (marker, version) in [
        ("schema", 2),
        ("thread_fences", 1),
        ("whatsapp_forms", 1),
        ("media_continuations", 1),
        ("worker_credentials", 1),
        (CUTOVER_BARRIER, 1),
    ] {
        assert_eq!(
            db.collection::<Document>(MIGRATIONS)
                .find_one(doc! {"_id":marker})
                .await?
                .unwrap()
                .get_i32("version")?,
            version
        );
    }
    assert!(restarted.was_worker(&digest).await?);
    db.collection::<Document>(USES)
        .create_index(
            IndexModel::builder()
                .keys(doc! {"first_used_at":1})
                .options(
                    IndexOptions::builder()
                        .expire_after(Duration::from_secs(3600))
                        .build(),
                )
                .build(),
        )
        .await?;
    assert!(MongoWorkerHistory::open(reader_db).await.is_err());
    assert!(migrate(db).await.is_err());
    Ok(())
}
