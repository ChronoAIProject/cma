//! Shared credentials do not relax Gateway schema or record ownership checks.
use cma_service_auth::mongodb_authority::MongoAuthorityMode::SharedSchema;
use cmaeg::records::mongo::{self, MongoRecords};
use mongodb::{
    bson::{doc, Document},
    options::{ClientOptions, Credential},
    Client, Database,
};
use std::collections::BTreeSet;

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn shared_schema_requires_opt_in_and_migration_and_preserves_foreign_collections() {
    let uri = std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_eg_shared_{}", uuid::Uuid::new_v4().simple());
    let result = exercise(&uri, &admin.database(&name)).await;
    admin.database(&name).drop().await.unwrap();
    result.unwrap();
}

async fn exercise(uri: &str, db: &Database) -> Result<(), Box<dyn std::error::Error>> {
    let password = uuid::Uuid::new_v4().to_string();
    db.run_command(
        doc! {"createUser":"shared-runtime","pwd":&password,"roles":[
            {"role":"readWrite","db":db.name()},{"role":"dbAdmin","db":db.name()}
        ]},
    )
    .await?;
    let mut options = ClientOptions::parse(uri).await?;
    options.credential = Some(
        Credential::builder()
            .username("shared-runtime".to_owned())
            .password(password)
            .source(db.name().to_owned())
            .build(),
    );
    let shared = Client::with_options(options)?.database(db.name());
    let foreign = ["agents", "im_images", "gateway_channel_records"];
    for name in foreign {
        db.run_command(doc! {"create":name,"validator":{"owner":{"$type":"string"}},"validationLevel":"strict","validationAction":"error"}).await?;
        db.collection::<Document>(name)
            .insert_one(doc! {"_id":"sentinel","owner":"foreign-owner","payload":"preserve"})
            .await?;
        db.run_command(doc! {"createIndexes":name,"indexes":[{"key":{"owner":1},"name":"foreign_owner","unique":true}]}).await?;
    }
    let before = snapshot(db, &foreign).await?;
    assert!(MongoRecords::open(shared.clone()).await.is_err());
    assert!(
        MongoRecords::open_with_authority(shared.clone(), SharedSchema)
            .await
            .is_err()
    );
    assert_eq!(db.list_collection_names().await?.len(), foreign.len());

    mongo::migrate(&shared).await?;
    mongo::migrate(&shared).await?;
    assert!(MongoRecords::open(shared.clone()).await.is_err());
    assert!(MongoRecords::open_with_authority(db.clone(), SharedSchema)
        .await
        .is_err());
    let records = MongoRecords::open_with_authority(shared.clone(), SharedSchema).await?;
    assert_eq!(
        records
            .put("shared-record", "alice", b"first", None)
            .await?,
        1
    );
    assert_eq!(records.get("shared-record").await?.blob, b"first");
    assert!(records
        .put("shared-record", "bob", b"forged", Some(1))
        .await
        .is_err());
    assert!(records.list(Some("bob"), None, 50).await?.is_empty());
    assert_eq!(
        records
            .put("shared-record", "alice", b"updated", Some(1))
            .await?,
        2
    );
    assert!(records.delete("shared-record", 1).await.is_err());
    assert!(shared
        .collection::<Document>(mongo::RECORDS)
        .insert_one(doc! {"_id":"invalid"})
        .await
        .is_err());

    shared
        .collection::<Document>(mongo::MIGRATIONS)
        .delete_one(doc! {"_id":"schema"})
        .await?;
    assert!(
        MongoRecords::open_with_authority(shared.clone(), SharedSchema)
            .await
            .is_err()
    );
    mongo::migrate(&shared).await?;
    let reopened = MongoRecords::open_with_authority(shared.clone(), SharedSchema).await?;
    assert_eq!(reopened.get("shared-record").await?.blob, b"updated");
    assert!(reopened.delete("shared-record", 2).await?);
    assert!(reopened.get("shared-record").await.is_err());
    super::record_domains::verify(db, reopened).await?;
    db.collection::<Document>(mongo::INTERACTIONS)
        .drop_index("expires_at_1")
        .await?;
    assert!(
        MongoRecords::open_with_authority(shared.clone(), SharedSchema)
            .await
            .is_err()
    );
    mongo::migrate(&shared).await?;
    MongoRecords::open_with_authority(shared, SharedSchema).await?;
    assert_eq!(snapshot(db, &foreign).await?, before);
    let expected: BTreeSet<_> = foreign
        .into_iter()
        .chain(mongo::COLLECTIONS.iter().copied())
        .map(str::to_owned)
        .collect();
    assert_eq!(
        db.list_collection_names()
            .await?
            .into_iter()
            .collect::<BTreeSet<_>>(),
        expected
    );
    Ok(())
}

pub(super) async fn snapshot(
    db: &Database,
    names: &[&str],
) -> Result<Vec<Document>, mongodb::error::Error> {
    let mut state = Vec::new();
    for name in names {
        for command in [
            doc! {"listCollections":1,"filter":{"name":*name}},
            doc! {"listIndexes":*name},
            doc! {"find":*name,"filter":{}},
        ] {
            let reply = db.run_command(command).await?;
            state.push(doc! {"documents":reply.get_document("cursor").unwrap().get_array("firstBatch").unwrap().clone()});
        }
    }
    Ok(state)
}
