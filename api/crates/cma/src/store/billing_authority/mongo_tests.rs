use super::*;
use crate::store::{
    settings::SETTINGS, AuthEncryptionKey, MongoPlatformSettingsStore, MongoStores,
};
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{Arc, Mutex};

mod cancellation;
mod startup;

type Commands = Arc<Mutex<Vec<Document>>>;
struct Fixture {
    client: Client,
    db: Database,
    commands: Commands,
    app: String,
}

impl Fixture {
    async fn new() -> Self {
        let uri =
            std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
        let name = format!("billing_reader_{}", uuid::Uuid::new_v4().simple());
        let app = format!("billing-reader-{}", uuid::Uuid::new_v4());
        let commands = Arc::new(Mutex::new(Vec::new()));
        let observed = commands.clone();
        let database = name.clone();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                if event.db == database {
                    observed.lock().unwrap().push(event.command);
                }
            }
        }));
        let client = Client::with_options(options).unwrap();
        let db = client.database(&name);
        Self {
            client,
            db,
            commands,
            app,
        }
    }

    async fn seed(&self) {
        self.db
            .collection(SETTINGS)
            .insert_many(format_v1::proposed_documents())
            .await
            .unwrap();
    }

    async fn full_stores(&self) -> Result<MongoStores, StoreError> {
        MongoStores::open(
            self.client.clone(),
            self.db.name(),
            "billing-fixture",
            &key(),
            Duration::from_secs(90 * 86400),
            Arc::new(crate::store::checkpoint_storage::RetiredCheckpointStorage),
            ("runtime-fixture", None),
        )
        .await
    }
}

fn key() -> AuthEncryptionKey {
    AuthEncryptionKey::from_base64(&base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        [0_u8; 32],
    ))
    .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn absent_reader_and_repeated_readiness_create_no_authority_or_namespace() {
    let f = Fixture::new().await;
    for _ in 0..2 {
        require_legacy_compatible(&f.db).await.unwrap();
    }
    assert!(f.db.list_collection_names().await.unwrap().is_empty());
    let commands = f.commands.lock().unwrap().clone();
    for command in commands {
        assert!(command.contains_key("find") || command.contains_key("listCollections"));
        if command.contains_key("find") {
            assert_eq!(command.get_str("find").unwrap(), SETTINGS);
            assert_eq!(command.get_i64("limit").unwrap(), 3);
            assert_eq!(command["readConcern"]["level"].as_str(), Some("snapshot"));
        }
    }
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn generic_guard_recognizes_only_exact_supported_pair_and_predecessor_refuses_it() {
    let f = Fixture::new().await;
    f.seed().await;
    let collection = f.db.collection::<Document>(SETTINGS);
    let predecessor_filter = doc! {"$or":[
        {"document_schema_version":{"$nin":[1_i64]}},
        {"document_schema_version":{"$not":{"$type":["int","long"]}}},
        {"document_schema_version":{"$type":"array"}}
    ]};
    assert_eq!(
        collection
            .count_documents(predecessor_filter)
            .await
            .unwrap(),
        2
    );
    crate::store::schema::refuse_unknown_versions(&f.db, &[SETTINGS])
        .await
        .unwrap();
    for row in [
        doc! {"_id":"unknown-sidecar","document_schema_version":2_i64},
        doc! {"_id":"unknown-sidecar","document_schema_version":[2_i64]},
    ] {
        collection.insert_one(row).await.unwrap();
        assert!(
            crate::store::schema::refuse_unknown_versions(&f.db, &[SETTINGS])
                .await
                .is_err()
        );
        collection
            .delete_one(doc! {"_id":"unknown-sidecar"})
            .await
            .unwrap();
    }
    for change in [
        doc! {"mode":"nyxid"},
        doc! {"epoch":1_i64},
        doc! {"extra":true},
        doc! {"document_schema_version":[2_i64]},
    ] {
        collection
            .update_one(doc! {"_id":format_v1::STATE_ID}, doc! {"$set":change})
            .await
            .unwrap();
        assert!(
            crate::store::schema::refuse_unknown_versions(&f.db, &[SETTINGS])
                .await
                .is_err()
        );
        collection
            .replace_one(
                doc! {"_id":format_v1::STATE_ID},
                format_v1::proposed_documents()[1].clone(),
            )
            .await
            .unwrap();
    }
    collection
        .delete_one(doc! {"_id":format_v1::FLOOR_ID})
        .await
        .unwrap();
    assert!(MongoPlatformSettingsStore::open(&f.client, f.db.name())
        .await
        .is_err());
    f.db.drop().await.unwrap();
}
