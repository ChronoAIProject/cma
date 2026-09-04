use std::time::Duration;

use layer2_core::StoreError;
use mongodb::bson::{doc, Document};
use mongodb::Client;

use super::activity::{MongoActivityStore, ACTIVITY};
use super::auth::{MongoAuthStore, AUTH_COLLECTIONS};
use super::directory::{MongoUserDirectoryStore, USERS};
use super::encryption::AuthEncryptionKey;
use super::operation::{MongoOperationStore, OPERATIONS};
use super::settings::{MongoPlatformSettingsStore, SETTINGS};

async fn database() -> (Client, String) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
    let client = Client::with_uri_str(uri)
        .await
        .expect("test Mongo connects");
    let name = format!("cma_schema_test_{}", uuid::Uuid::new_v4().simple());
    (client, name)
}

async fn insert(client: &Client, database: &str, collection: &str, document: Document) {
    client
        .database(database)
        .collection::<Document>(collection)
        .insert_one(document)
        .await
        .unwrap();
}

async fn clear(client: &Client, database: &str, collection: &str) {
    client
        .database(database)
        .collection::<Document>(collection)
        .delete_many(doc! {})
        .await
        .unwrap();
}

fn auth_key() -> AuthEncryptionKey {
    AuthEncryptionKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn retained_stores_initialize_and_refuse_unsupported_schema_versions() {
    let (client, name) = database().await;

    MongoAuthStore::open(&client, &name, "test", &auth_key())
        .await
        .unwrap();
    MongoActivityStore::open(&client, &name, Duration::from_secs(86_400))
        .await
        .unwrap();
    MongoUserDirectoryStore::open(&client, &name).await.unwrap();
    MongoOperationStore::open(&client, &name).await.unwrap();
    MongoPlatformSettingsStore::open(&client, &name)
        .await
        .unwrap();
    let names = client
        .database(&name)
        .list_collection_names()
        .await
        .unwrap();
    for collection in AUTH_COLLECTIONS
        .iter()
        .copied()
        .chain([ACTIVITY, USERS, OPERATIONS, SETTINGS])
    {
        assert!(
            names.iter().any(|name| name == collection),
            "startup did not create {collection}"
        );
    }

    for collection in AUTH_COLLECTIONS {
        insert(
            &client,
            &name,
            collection,
            doc! { "_id": format!("bad-{collection}"), "document_schema_version": 2_i64 },
        )
        .await;
        assert!(matches!(
            MongoAuthStore::open(&client, &name, "test", &auth_key()).await,
            Err(StoreError::Malformed(_))
        ));
        clear(&client, &name, collection).await;
    }

    insert(&client, &name, ACTIVITY, doc! { "_id": "missing" }).await;
    assert!(matches!(
        MongoActivityStore::open(&client, &name, Duration::from_secs(86_400)).await,
        Err(StoreError::Malformed(_))
    ));
    clear(&client, &name, ACTIVITY).await;

    insert(
        &client,
        &name,
        USERS,
        doc! { "_id": "array-version", "document_schema_version": [1_i64] },
    )
    .await;
    assert!(matches!(
        MongoUserDirectoryStore::open(&client, &name).await,
        Err(StoreError::Malformed(_))
    ));

    insert(
        &client,
        &name,
        OPERATIONS,
        doc! { "_id": "wrong-type", "document_schema_version": "1" },
    )
    .await;
    assert!(matches!(
        MongoOperationStore::open(&client, &name).await,
        Err(StoreError::Malformed(_))
    ));

    insert(
        &client,
        &name,
        SETTINGS,
        doc! { "_id": "future", "document_schema_version": 2_i64 },
    )
    .await;
    assert!(matches!(
        MongoPlatformSettingsStore::open(&client, &name).await,
        Err(StoreError::Malformed(_))
    ));

    client.database(&name).drop().await.unwrap();
}
