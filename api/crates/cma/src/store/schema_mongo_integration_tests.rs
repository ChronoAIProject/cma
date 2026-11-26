use std::time::{Duration, SystemTime};

use futures_util::TryStreamExt as _;
use layer2_core::StoreError;
use mongodb::bson::{doc, DateTime, Document};
use mongodb::options::IndexOptions;
use mongodb::{Client, Database, IndexModel};

use super::activity::{MongoActivityStore, ACTIVITY};
use super::auth::{MongoAuthStore, AUTH_COLLECTIONS};
use super::directory::{MongoUserDirectoryStore, USERS};
use super::encryption::{AuthCipher, AuthEncryptionKey};
use super::operation::{MongoOperationStore, OPERATIONS};
use super::settings::{MongoPlatformSettingsStore, SETTINGS};
use super::MongoStores;

mod activity_batch;
mod activity_pages;
mod activity_retention;
mod auto_pause_compatibility;
mod released_authority;
mod routing_source_fence;
mod session_versions;
mod startup_ddl;
mod visitor_defaults;

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
        .chain([USERS, OPERATIONS, SETTINGS])
    {
        assert!(
            names.iter().any(|name| name == collection),
            "startup did not create {collection}"
        );
    }

    assert!(
        !names.iter().any(|name| name == ACTIVITY),
        "activity startup must not bootstrap its collection"
    );

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

fn legacy_ciphertext(cipher: &AuthCipher, kind: &str, id: &str, field: &str) -> Document {
    mongodb::bson::serialize_to_document(
        &cipher
            .seal(kind, id.as_bytes(), field, "historical fixture")
            .unwrap(),
    )
    .unwrap()
}

async fn index_documents(database: &Database, collection: &str) -> Vec<Document> {
    let indexes: Vec<IndexModel> = database
        .collection::<Document>(collection)
        .list_indexes()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let mut documents: Vec<_> = indexes
        .iter()
        .map(|index| mongodb::bson::serialize_to_document(index).unwrap())
        .collect();
    documents.sort_by(|left, right| {
        left.get_str("name")
            .unwrap()
            .cmp(right.get_str("name").unwrap())
    });
    documents
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB replica set"]
async fn archived_legacy_collections_preserve_custody_and_allow_full_new_store_bootstrap() {
    const CONFLICTS: [&str; 8] = [
        ACTIVITY,
        "auth_login_attempts",
        "auth_subjects",
        OPERATIONS,
        SETTINGS,
        "trigger_launch_events",
        "trigger_launches",
        USERS,
    ];
    const ARCHIVE_PREFIX: &str = "legacy_reset_";
    const SUBJECT_EXPIRY: &str = "auth_subject_expiry";
    const OLD_EVENT_INDEX: &str = "trigger_launch_events_by_sequence";
    let (client, name) = database().await;
    let database = client.database(&name);
    let cipher = AuthCipher::new("schema-archive-test", &auth_key()).unwrap();
    let retained_until = DateTime::from_system_time(SystemTime::now() + Duration::from_secs(3600));
    let subject = doc! {
        "_id": "legacy-owner",
        "auth_schema_version": 6_i64,
        "tokens": {
            "access_token": legacy_ciphertext(
                &cipher, "subject-auth", "legacy-owner", "tokens.access_token"
            ),
            "refresh_token": legacy_ciphertext(
                &cipher, "subject-auth", "legacy-owner", "tokens.refresh_token"
            ),
        },
        "delete_after": retained_until,
    };
    let mut archived = Vec::new();
    for collection in CONFLICTS {
        let document = if collection == "auth_subjects" {
            subject.clone()
        } else {
            doc! { "_id": "historical-row", "legacy_field": collection }
        };
        insert(&client, &name, collection, document.clone()).await;
        archived.push((collection, document));
    }
    database
        .collection::<Document>("auth_subjects")
        .create_index(
            IndexModel::builder()
                .keys(doc! { "delete_after": 1 })
                .options(
                    IndexOptions::builder()
                        .name(SUBJECT_EXPIRY.to_owned())
                        .expire_after(Duration::ZERO)
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
    let events = database.collection::<Document>("trigger_launch_events");
    events
        .create_index(
            IndexModel::builder()
                .keys(doc! { "launch_id": 1, "sequence": 1 })
                .options(
                    IndexOptions::builder()
                        .name(OLD_EVENT_INDEX.to_owned())
                        .unique(true)
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
    let first_event = doc! {
        "_id": "new-event-one", "document_schema_version": 1_i64,
        "launch_id": "new-launch", "seq": 1_i64,
    };
    let second_event = doc! {
        "_id": "new-event-two", "document_schema_version": 1_i64,
        "launch_id": "new-launch", "seq": 2_i64,
    };
    // The old unique index treats both missing `sequence` values as null.
    events.insert_one(first_event.clone()).await.unwrap();
    let conflict = events.insert_one(second_event.clone()).await.unwrap_err();
    assert!(super::sandbox_mongo::is_duplicate_key(&conflict));
    events
        .delete_one(doc! { "_id": "new-event-one" })
        .await
        .unwrap();

    let custody = doc! {
        "_id": "sesn_legacy", "auth_schema_version": 6_i64,
        "retired": false, "lease": null,
        "current": {
            "id": "provider-current",
            "value": legacy_ciphertext(
                &cipher, "session-credential", "sesn_legacy", "current.value"
            ),
        },
        "pending": {
            "id": "provider-pending",
            "value": legacy_ciphertext(
                &cipher, "session-credential", "sesn_legacy", "pending.value"
            ),
        },
        "revoke_debt": [{
            "id": "provider-debt", "owner": "legacy-owner",
            "revocation_credential": legacy_ciphertext(
                &cipher, "session-credential", "sesn_legacy",
                "revoke_debt.provider-debt.revocation_credential"
            ),
        }],
    };
    let retained = [
        ("auth_session_credentials", custody),
        (
            "session_tombstones",
            doc! { "_id": "sesn_legacy", "_cma_write_protocol": 1_i32 },
        ),
        (
            "chat_hosts",
            doc! {
                "_id": "chho_legacy",
                "record": { "checkpoint": null, "last_transition_claim": "chcp_legacy" },
            },
        ),
        (
            "sessions_v3",
            doc! {
                "_id": "sesn_legacy", "_cma_write_protocol": 2_i32,
                "context_ref": "sessions/sesn_legacy/context.jsonl",
                "pending_handover": { "snapshot_ref": "sessions/sesn_legacy/handover.snapshot" },
            },
        ),
    ];
    for (collection, document) in &retained {
        insert(&client, &name, collection, document.clone()).await;
    }
    database
        .create_collection("sessions")
        .view_on("sessions_v3")
        .pipeline(Vec::<Document>::new())
        .await
        .unwrap();
    assert!(matches!(
        MongoAuthStore::open(&client, &name, "schema-archive-test", &auth_key()).await,
        Err(StoreError::Malformed(_))
    ));

    for (collection, _) in &archived {
        let original_indexes = index_documents(&database, collection).await;
        let archive_name = format!("{ARCHIVE_PREFIX}{collection}");
        client
            .database("admin")
            .run_command(doc! {
                "renameCollection": format!("{name}.{collection}"),
                "to": format!("{name}.{archive_name}"),
                "dropTarget": false,
                "writeConcern": { "w": "majority" },
            })
            .await
            .unwrap();
        assert_eq!(
            index_documents(&database, &archive_name).await,
            original_indexes
        );
    }
    let archived_subjects = format!("{ARCHIVE_PREFIX}auth_subjects");
    let subject_indexes = index_documents(&database, &archived_subjects).await;
    let saved_expiry = subject_indexes
        .iter()
        .find(|index| index.get_str("name").unwrap() == SUBJECT_EXPIRY)
        .unwrap();
    assert_eq!(
        saved_expiry.get_document("key").unwrap(),
        &doc! { "delete_after": 1 }
    );
    assert!(matches!(
        saved_expiry.get("expireAfterSeconds"),
        Some(mongodb::bson::Bson::Int32(0) | mongodb::bson::Bson::Int64(0))
    ));
    database
        .collection::<Document>(&archived_subjects)
        .drop_index(SUBJECT_EXPIRY)
        .await
        .unwrap();
    assert_eq!(
        index_documents(&database, &archived_subjects).await,
        subject_indexes
            .into_iter()
            .filter(|index| index.get_str("name").unwrap() != SUBJECT_EXPIRY)
            .collect::<Vec<_>>()
    );

    // This fixture owns schema migration with an unrestricted disposable
    // credential. Production connect verifies its runtime role before DDL.
    let stores = MongoStores::open(
        client.clone(),
        &name,
        "schema-archive-test",
        &auth_key(),
        Duration::from_secs(86_400),
        super::checkpoint_storage::test_storage(&name),
        ("runtime-schema-test", None),
    )
    .await
    .unwrap();
    stores.ping().await.unwrap();
    for collection in CONFLICTS {
        assert_eq!(
            database
                .collection::<Document>(collection)
                .count_documents(doc! {})
                .await
                .unwrap(),
            u64::from(collection == SETTINGS),
            "unexpected records after bootstrap in {collection}"
        );
    }
    // Admission needs a settings fence before its first concurrent transaction.
    // It must contain only predecessor-readable defaults, never an active policy.
    assert_eq!(
        database
            .collection::<Document>(SETTINGS)
            .find_one(doc! {})
            .await
            .unwrap(),
        Some(doc! {
            "_id": "platform",
            "document_schema_version": super::sandbox_mongo::DOCUMENT_SCHEMA_VERSION,
            "revision": 0_i64,
            "settings": mongodb::bson::serialize_to_bson(&layer2_core::PlatformSettings::default()).unwrap(),
        })
    );
    events
        .insert_many([first_event, second_event])
        .await
        .unwrap();
    assert_eq!(events.count_documents(doc! {}).await.unwrap(), 2);
    assert!(index_documents(&database, "trigger_launch_events")
        .await
        .iter()
        .all(|index| index.get_str("name").unwrap() != OLD_EVENT_INDEX));
    let repeated_sequence = doc! {
        "_id": "new-event-duplicate", "document_schema_version": 1_i64,
        "launch_id": "new-launch", "seq": 2_i64,
    };
    let conflict = events.insert_one(repeated_sequence).await.unwrap_err();
    assert!(super::sandbox_mongo::is_duplicate_key(&conflict));

    for (collection, original) in &archived {
        let archive_name = format!("{ARCHIVE_PREFIX}{collection}");
        let preserved = database
            .collection::<Document>(&archive_name)
            .find_one(doc! { "_id": original.get("_id").unwrap().clone() })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&preserved, original);
    }
    for (collection, original) in &retained {
        let preserved = database
            .collection::<Document>(collection)
            .find_one(doc! { "_id": original.get("_id").unwrap().clone() })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&preserved, original);
    }
    assert_eq!(
        database
            .collection::<Document>("sessions")
            .find_one(doc! {})
            .await
            .unwrap(),
        Some(retained[3].1.clone())
    );
    drop(stores);
    database.drop().await.unwrap();
}
