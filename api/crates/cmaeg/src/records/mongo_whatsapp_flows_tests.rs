//! Actual Mongo validators and grants, independent of permissive HTTP fixtures.
use super::*;
use crate::records::{RecordClient, RecordKind};
use mongodb::{
    options::{ClientOptions, Credential},
    Client,
};

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn optional_forms_storage_has_exact_grants_indexes_and_persistent_retention() {
    let uri = std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_forms_contract_{}", uuid::Uuid::new_v4().simple());
    let result = exercise(&uri, &admin.database(&name)).await;
    admin.database(&name).drop().await.unwrap();
    result.unwrap();
}
async fn exercise(uri: &str, db: &Database) -> Result<(), Box<dyn std::error::Error>> {
    let name = db.name();
    super::super::migrate(db).await?;
    db.run_command(super::super::role(name, false)).await?;
    db.run_command(role(name, false)).await?;
    db.run_command(role(name, true)).await?;
    let password = uuid::Uuid::new_v4().to_string();
    db.run_command(
        doc! {"createUser":"runtime","pwd":&password,"roles":[{"role":"cma_eg_runtime","db":name}]},
    )
    .await?;
    db.run_command(doc! {"createUser":"forms-migrator","pwd":&password,"roles":[{"role":"cma_eg_whatsapp_forms_migrator","db":name}]}).await?;
    let connect = |username: &str| {
        let username = username.to_owned();
        let password = password.clone();
        async move {
            let mut options = ClientOptions::parse(uri).await?;
            options.credential = Some(
                Credential::builder()
                    .username(username)
                    .password(password)
                    .source(name.to_owned())
                    .build(),
            );
            Client::with_options(options)
        }
    };
    let runtime = connect("runtime").await?;
    let records = MongoRecords::open(runtime.database(name)).await?;
    assert!(records.whatsapp_flows_ready().await.is_err());
    db.run_command(doc! {"grantRolesToUser":"runtime","roles":[{"role":"cma_eg_whatsapp_forms_runtime","db":name}]}).await?;
    let runtime = connect("runtime").await?;
    let records = MongoRecords::open(runtime.database(name)).await?;
    assert!(records.whatsapp_flows_ready().await.is_err());
    let migrator = connect("forms-migrator").await?;
    migrate(&migrator.database(name)).await?;
    migrate(&migrator.database(name)).await?;
    records.whatsapp_flows_ready().await?;
    // Both optional stores must coexist through either migration order and base reruns.
    db.run_command(super::super::thread_fences_role(name, false))
        .await?;
    db.run_command(doc! {"grantRolesToUser":"runtime","roles":[{"role":"cma_eg_thread_fences_runtime","db":name}]}).await?;
    super::super::migrate_thread_fences(db).await?;
    migrate(&migrator.database(name)).await?;
    super::super::migrate(db).await?;
    super::super::migrate_thread_fences(db).await?;
    let runtime = connect("runtime").await?;
    let records = MongoRecords::open(runtime.database(name)).await?;
    records.whatsapp_flows_ready().await?;
    records.thread_fences_ready().await?;
    for (marker, version) in [("schema", 2), ("whatsapp_forms", 1), ("thread_fences", 1)] {
        assert_eq!(
            db.collection::<Document>(MIGRATIONS)
                .find_one(doc! {"_id":marker})
                .await?
                .unwrap()
                .get_i32("version")?,
            version
        );
    }
    assert_eq!(
        db.collection::<Document>(MIGRATIONS)
            .find_one(doc! {"_id":"schema"})
            .await?
            .unwrap()
            .get_i32("version")?,
        2
    );
    // Base migration is compatible with an already installed optional marker.
    super::super::migrate(db).await?;
    records.whatsapp_flows_ready().await?;
    // Existing optional validators need no unrelated GitHub migration to keep writing.
    let legacy_validator = doc! {"$jsonSchema":{"bsonType":"object","additionalProperties":false,
    "required":["_id","owner","blob","version","schema_version"],"properties":{
        "_id":{"bsonType":"string"},"owner":{"bsonType":"string"},"blob":{"bsonType":"binData"},
        "version":{"bsonType":"long"},"schema_version":{"enum":[1]}}}};
    for collection in [WHATSAPP_FLOWS, super::super::THREAD_FENCES] {
        db.run_command(doc! {"collMod":collection,"validator":legacy_validator.clone()})
            .await?;
    }
    let flows = records.for_kind(RecordKind::WhatsappFlow);
    let id = uuid::Uuid::new_v4().to_string();
    assert_eq!(flows.put(&id, "owner", b"persistent", None).await?, 1);
    assert_eq!(records.put(&id, "owner", b"channel", None).await?, 1);
    let fences = records.for_kind(RecordKind::ThreadFence);
    assert_eq!(fences.put(&id, "owner", b"thread", None).await?, 1);
    assert_eq!(flows.get(&id).await?.blob, b"persistent");
    assert_eq!(records.get(&id).await?.blob, b"channel");
    assert_eq!(fences.get(&id).await?.blob, b"thread");
    assert!(records
        .for_kind(RecordKind::Interaction)
        .put(&id, "owner", b"invalid", None)
        .await
        .is_err());
    assert!(flows
        .put_expiring(&id, "owner", b"invalid", Some(1), Some(1_900_000_000))
        .await
        .is_err());
    assert!(flows.delete(&id, 1).await.is_err());
    assert!(runtime
        .database(name)
        .collection::<Document>(WHATSAPP_FLOWS)
        .update_one(
            doc! {"_id":&id},
            doc! {"$set":{"expires_at":mongodb::bson::DateTime::now()}}
        )
        .await
        .is_err());
    assert!(runtime
        .database(name)
        .run_command(doc! {"collMod":WHATSAPP_FLOWS,"validationLevel":"off"})
        .await
        .is_err());
    let row = db
        .collection::<Document>(WHATSAPP_FLOWS)
        .find_one(doc! {"_id":&id})
        .await?
        .unwrap();
    assert!(!row.contains_key("expires_at"));
    crate::whatsapp::forms::durable_tests::exercise_mongo(records.clone()).await;
    let client =
        RecordClient::new("https://records.example", "fixture")?.with_database(records.clone());
    assert!(client.whatsapp_flows_ready().await);
    // Each partial migration state disables Forms without breaking base startup.
    db.collection::<Document>(MIGRATIONS)
        .delete_one(doc! {"_id":"whatsapp_forms"})
        .await?;
    assert!(!client.whatsapp_flows_ready().await);
    MongoRecords::open(runtime.database(name)).await?;
    migrate(&migrator.database(name)).await?;
    db.collection::<Document>(WHATSAPP_FLOWS)
        .drop_index("owner_1__id_1")
        .await?;
    assert!(!client.whatsapp_flows_ready().await);
    migrate(&migrator.database(name)).await?;
    db.collection::<Document>(WHATSAPP_FLOWS)
        .create_index(
            IndexModel::builder()
                .keys(doc! {"expires_at":1})
                .options(
                    mongodb::options::IndexOptions::builder()
                        .expire_after(std::time::Duration::ZERO)
                        .build(),
                )
                .build(),
        )
        .await?;
    assert!(!client.whatsapp_flows_ready().await);
    assert!(migrate(&migrator.database(name)).await.is_err());
    db.collection::<Document>(WHATSAPP_FLOWS)
        .drop_index("expires_at_1")
        .await?;
    assert!(client.whatsapp_flows_ready().await);
    let mut partial = role(name, false);
    partial.get_array_mut("privileges")?[0]
        .as_document_mut()
        .unwrap()
        .insert("actions", vec!["find", "insert", "listIndexes"]);
    db.run_command(doc! {"updateRole":"cma_eg_whatsapp_forms_runtime","privileges":partial.get_array("privileges")?.clone(),"roles":[]}).await?;
    let partial_runtime = connect("runtime").await?;
    let partial_records = MongoRecords::open(partial_runtime.database(name)).await?;
    assert!(partial_records.whatsapp_flows_ready().await.is_err());
    Ok(())
}
