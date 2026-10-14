//! Explicit shared-schema mode still validates migrations and service writes.
use cma_image_manager::{model::CreateImage, schema, store::Store};
use cma_service_auth::mongodb_authority::MongoAuthorityMode::SharedSchema;
use mongodb::{
    bson::{doc, Document},
    options::{ClientOptions, Credential},
    Client, Database,
};
use std::collections::BTreeSet;

#[tokio::test]
#[ignore = "requires the authenticated image-manager Mongo contract fixture"]
async fn shared_schema_requires_opt_in_and_migration_and_preserves_foreign_collections() {
    let uri = std::env::var("CMA_IM_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_im_shared_{}", uuid::Uuid::new_v4().simple());
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
    let foreign = ["agents", "eg_channel_records", "gateway_channel_records"];
    for name in foreign {
        db.run_command(doc! {"create":name,"validator":{"owner":{"$type":"string"}},"validationLevel":"strict","validationAction":"error"}).await?;
        db.collection::<Document>(name)
            .insert_one(doc! {"_id":"sentinel","owner":"foreign-owner","payload":"preserve"})
            .await?;
        db.run_command(doc! {"createIndexes":name,"indexes":[{"key":{"owner":1},"name":"foreign_owner","unique":true}]}).await?;
    }
    let before = snapshot(db, &foreign).await?;
    assert!(Store::open(shared.clone()).await.is_err());
    assert!(Store::open_with_authority(shared.clone(), SharedSchema)
        .await
        .is_err());
    assert_eq!(db.list_collection_names().await?.len(), foreign.len());

    schema::migrate(&shared).await?;
    schema::migrate(&shared).await?;
    assert!(Store::open(shared.clone()).await.is_err());
    assert!(Store::open_with_authority(db.clone(), SharedSchema)
        .await
        .is_err());
    let store = Store::open_with_authority(shared.clone(), SharedSchema).await?;
    let image = store
        .create_image(
            "alice",
            "shared-create",
            CreateImage {
                name: "first".into(),
                description: "".into(),
            },
        )
        .await?;
    assert_eq!(store.image("alice", &image.id, true).await?.id, image.id);
    assert!(store.image("bob", &image.id, false).await.is_err());
    let changed = store
        .update_image(
            "alice",
            &image.id,
            image.revision,
            CreateImage {
                name: "updated".into(),
                description: "same database".into(),
            },
        )
        .await?;
    assert_eq!(changed.spec.name, "updated");
    assert!(store
        .update_image("alice", &image.id, image.revision, changed.spec.clone())
        .await
        .is_err());
    assert!(
        store
            .retire("alice", &image.id, changed.revision)
            .await?
            .retired
    );
    assert!(shared
        .collection::<Document>(schema::IMAGES)
        .insert_one(doc! {"_id":"invalid"})
        .await
        .is_err());

    shared
        .collection::<Document>(schema::MIGRATIONS)
        .delete_one(doc! {"_id":"schema"})
        .await?;
    assert!(Store::open_with_authority(shared.clone(), SharedSchema)
        .await
        .is_err());
    schema::migrate(&shared).await?;
    let reopened = Store::open_with_authority(shared, SharedSchema).await?;
    assert!(reopened.image("alice", &image.id, true).await?.retired);
    assert_eq!(snapshot(db, &foreign).await?, before);
    let expected: BTreeSet<_> = foreign
        .into_iter()
        .chain(schema::COLLECTIONS.iter().copied())
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

async fn snapshot(db: &Database, names: &[&str]) -> Result<Vec<Document>, mongodb::error::Error> {
    let mut state = Vec::new();
    for name in names {
        for command in [
            doc! {"listCollections":1,"filter":{"name":*name}},
            doc! {"listIndexes":*name},
            doc! {"find":*name,"filter":{}},
        ] {
            let reply = db.run_command(command).await?;
            // Cursor metadata varies; actual documents, validators and index
            // definitions must remain byte-for-byte structurally identical.
            state.push(doc! {"documents":reply.get_document("cursor").unwrap().get_array("firstBatch").unwrap().clone()});
        }
    }
    Ok(state)
}
