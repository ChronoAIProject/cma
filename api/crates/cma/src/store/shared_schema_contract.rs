use super::*;
use cma_service_auth::mongodb_authority::MongoAuthorityMode;
use mongodb::{
    options::{ClientOptions, Credential},
    Client,
};

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn real_shared_database_bootstrap_preserves_foreign_schemas() {
    let uri = std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_shared_contract_{}", uuid::Uuid::new_v4().simple());
    let db = admin.database(&name);
    let password = uuid::Uuid::new_v4().to_string();
    db.run_command(doc! {"createUser":"shared-runtime","pwd":&password,"roles":[{"role":"readWrite","db":&name},{"role":"dbAdmin","db":&name}]}).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.credential = Some(
        Credential::builder()
            .username("shared-runtime".to_owned())
            .password(password)
            .source(name.clone())
            .build(),
    );
    let client = Client::with_options(options).unwrap();
    let shared = client.database(&name);
    assert!(verify(&shared).await.is_err());
    assert!(verify_with_authority(&db, MongoAuthorityMode::SharedSchema)
        .await
        .is_err());
    verify_with_authority(&shared, MongoAuthorityMode::SharedSchema)
        .await
        .unwrap();
    let foreign = [
        "im_images",
        "im_builds",
        "im_auth_sessions",
        "im_schema_migrations",
        "eg_channel_records",
        "eg_connection_records",
        "eg_interaction_records",
        "eg_auth_sessions",
        "eg_schema_migrations",
        "gateway_channel_records",
    ];
    let sentinel = doc! {"_id":"foreign", "schema_version":987, "payload":"owned-elsewhere"};
    for collection in foreign {
        db.create_collection(collection)
            .validator(doc! {"payload":{"$eq":"owned-elsewhere"}})
            .await
            .unwrap();
        db.collection::<Document>(collection)
            .insert_one(sentinel.clone())
            .await
            .unwrap();
    }
    let key = crate::store::AuthEncryptionKey::from_base64(&base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        [7_u8; 32],
    ))
    .unwrap();
    for _ in 0..2 {
        let stores = crate::store::MongoStores::open(
            client.clone(),
            &name,
            "shared-bootstrap",
            &key,
            std::time::Duration::from_secs(86400),
            std::sync::Arc::new(super::super::checkpoint_storage::RetiredCheckpointStorage),
            ("runtime-shared-contract", None),
        )
        .await
        .unwrap();
        stores.ping().await.unwrap();
    }
    let names = db.list_collection_names().await.unwrap();
    assert!(names
        .iter()
        .all(|collection| COLLECTIONS.contains(&collection.as_str())
            || foreign.contains(&collection.as_str())));
    for collection in foreign {
        let rows = db.collection::<Document>(collection);
        assert_eq!(
            rows.find_one(doc! {"_id":"foreign"}).await.unwrap(),
            Some(sentinel.clone())
        );
        assert_eq!(rows.count_documents(doc! {}).await.unwrap(), 1);
        assert!(
            rows.insert_one(doc! {"_id":"forbidden", "payload":"changed"})
                .await
                .is_err(),
            "foreign validator changed: {collection}"
        );
    }
    agent_lifecycle_contract::verify(&shared).await;
    db.drop().await.unwrap();
}
