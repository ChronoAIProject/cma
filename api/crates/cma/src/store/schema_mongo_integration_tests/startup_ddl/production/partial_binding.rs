use std::sync::{Arc, Mutex};

use layer2_core::{
    sandbox::{binding::BindingStore, store::SandboxStoreError},
    ActivityAction, ActivityOutcome, ActivityRecord, ActivityStore,
};
use mongodb::event::{command::CommandEvent, EventHandler};
use tokio::sync::oneshot;

use super::*;
use crate::store::{
    bindings::{
        insert_retained_binding_fixture,
        schema::{COLLECTIONS, LEGACY_AGENT_INDEX},
    },
    migrations::{apply_binding_indexes_for_test, apply_binding_indexes_with_pause_for_test},
    mongo::STARTUP_COMMANDS,
    schema_mongo_integration_tests::activity_retention::prepare_activity_schema,
};

const BINDINGS: &str = "provider_bindings";
const CONVERSATIONS: &str = "binding_conversations";
const MIGRATION: &str = "am-binding-indexes-v1";
const BARRIER_TIMEOUT: Duration = Duration::from_secs(20);

fn startup_mode(value: &str) -> StartupSchemaMode {
    StartupSchemaMode::from_lookup(&mut |key| {
        assert_eq!(key, "CHRONO_AM_STARTUP_SCHEMA_ENFORCEMENT_ENABLED");
        Ok(Some(value.into()))
    })
    .unwrap()
}

async fn connect_observed(
    uri: &url::Url,
    db: &Database,
    mode: StartupSchemaMode,
) -> Result<MongoStores, StoreError> {
    let commands = Arc::new(Mutex::new(Vec::new()));
    let recorded = commands.clone();
    let handler = EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            recorded
                .lock()
                .unwrap()
                .push((event.command_name, event.command));
        }
    });
    let stores = STARTUP_COMMANDS
        .scope(handler, connect(uri, db.name(), mode))
        .await;
    let commands = commands.lock().unwrap();
    assert!(commands.iter().any(|(name, command)| {
        name == "listIndexes" && command.get_str(name).ok() == Some(BINDINGS)
    }));
    if mode == StartupSchemaMode::Observe {
        assert!(commands.iter().any(|(name, command)| {
            name == "listIndexes" && command.get_str(name).ok() == Some(ACTIVITY)
        }));
    }
    for (name, command) in commands.iter() {
        assert_ne!(name, "dropDatabase");
        for collection in COLLECTIONS.iter().copied().chain([ACTIVITY]) {
            let namespace = format!("{}.{collection}", db.name());
            if name == "renameCollection" {
                assert_ne!(command.get_str(name).ok(), Some(namespace.as_str()));
                assert_ne!(command.get_str("to").ok(), Some(namespace.as_str()));
            }
            if command.get_str(name).ok() == Some(collection) {
                assert!(
                    matches!(name.as_str(), "find" | "listIndexes"),
                    "startup sent {name} to converted collection {collection}"
                );
            }
        }
    }
    stores
}

async fn retained_rows(db: &Database) -> Vec<Document> {
    let mut rows = Vec::new();
    for collection in [BINDINGS, CONVERSATIONS, ACTIVITY] {
        let contents = db
            .collection::<Document>(collection)
            .find(doc! {})
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        assert_eq!(contents.len(), 1, "{collection}");
        rows.extend(contents);
    }
    rows
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn production_startup_during_partial_binding_index_migration() {
    tokio::time::timeout(
        Duration::from_secs(120),
        exercise_partial_binding_migration(),
    )
    .await
    .expect("partial binding migration contract did not finish");
}

async fn exercise_partial_binding_migration() {
    let (admin, name) = database().await;
    let db = admin.database(&name);
    let uri = runtime_uri(&db).await;
    for &collection in COLLECTIONS {
        db.create_collection(collection).await.unwrap();
    }
    let (binding, conversation) = insert_retained_binding_fixture(&db).await;
    db.collection::<Document>(CONVERSATIONS)
        .create_index(
            IndexModel::builder()
                .keys(doc! {"record.agent_id": 1})
                .options(
                    IndexOptions::builder()
                        .name(LEGACY_AGENT_INDEX.to_owned())
                        .unique(true)
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
    prepare_activity_schema(&db, RETENTION).await;
    let activity = ActivityRecord::new(ActivityAction::Read, ActivityOutcome::Succeeded);
    MongoActivityStore::open(&admin, &name, RETENTION)
        .await
        .unwrap()
        .append(&activity)
        .await
        .unwrap();
    let rows = retained_rows(&db).await;
    let activity_indexes = index_documents(&db, ACTIVITY).await;
    let conversation_indexes = index_documents(&db, CONVERSATIONS).await;
    let ledger = db.collection::<Document>("agent_manager_migrations");

    let (created, first_index) = oneshot::channel();
    let (resume, release) = oneshot::channel();
    let migration_db = db.clone();
    let run = tokio::spawn(async move {
        apply_binding_indexes_with_pause_for_test(&migration_db, created, release).await
    });
    tokio::time::timeout(BARRIER_TIMEOUT, first_index)
        .await
        .expect("migrator did not acknowledge its first additive index")
        .unwrap();
    let partial = index_documents(&db, BINDINGS).await;
    let names: Vec<_> = partial
        .iter()
        .map(|index| index.get_str("name").unwrap())
        .collect();
    assert_eq!(names, ["_id_", "pending_1__id_1"]);
    assert_eq!(
        index_documents(&db, CONVERSATIONS).await,
        conversation_indexes
    );
    assert_eq!(
        ledger
            .count_documents(doc! {"migration": MIGRATION, "kind":"attempt", "outcome":"running"})
            .await
            .unwrap(),
        1,
    );

    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let strict = {
        let _recording = metrics::set_default_local_recorder(&recorder);
        connect_observed(&uri, &db, startup_mode("true")).await
    };
    assert!(
        matches!(strict, Err(StoreError::Unavailable(message)) if message.starts_with("binding schema drift"))
    );
    // The legacy index is also an enforcement finding. Verify startup saw the
    // missing uniqueness contract in the paused catalog.
    assert!(recorder
        .handle()
        .render()
        .contains("reason=\"missing_unique_index\""));
    let observing = connect_observed(&uri, &db, startup_mode("false"))
        .await
        .unwrap();
    observing.ping().await.unwrap();
    let store = &observing.sandbox_model;
    assert_eq!(
        store.binding(&binding.binding_id).await.unwrap(),
        Some(binding.clone())
    );
    assert_eq!(
        store
            .binding_conversation(
                &binding.binding_id,
                binding.generation,
                &conversation.conversation_id
            )
            .await
            .unwrap(),
        Some(conversation.clone())
    );
    let uniqueness_unverified = SandboxStoreError::Unavailable(
        "binding uniqueness is unverified; prepare schema and restart".into(),
    );
    assert_eq!(
        store.find_by_key_hash(&binding.key_sha256).await,
        Err(uniqueness_unverified.clone())
    );
    assert_eq!(
        store
            .binding_conversation_for_agent(
                &binding.binding_id,
                binding.generation,
                &conversation.agent_id
            )
            .await,
        Err(uniqueness_unverified.clone())
    );
    let mut contender = binding.clone();
    contender.binding_id = "binding-contender".into();
    assert_eq!(
        store.save_binding(None, contender).await,
        Err(uniqueness_unverified)
    );
    assert_eq!(retained_rows(&db).await, rows);
    assert_eq!(index_documents(&db, BINDINGS).await, partial);
    drop(observing);

    resume.send(()).unwrap();
    assert_eq!(run.await.unwrap().unwrap()["outcome"], "completed");
    let reopened = connect_observed(&uri, &db, startup_mode("false"))
        .await
        .unwrap();
    let store = &reopened.sandbox_model;
    assert_eq!(
        store.find_by_key_hash(&binding.key_sha256).await.unwrap(),
        Some(binding.clone())
    );
    assert_eq!(
        store
            .binding_conversation_for_agent(
                &binding.binding_id,
                binding.generation,
                &conversation.agent_id
            )
            .await
            .unwrap(),
        Some(conversation)
    );
    drop(reopened);
    assert_eq!(
        apply_binding_indexes_for_test(&db).await.unwrap()["outcome"],
        "already_completed"
    );
    assert_eq!(
        ledger
            .count_documents(doc! {"migration": MIGRATION, "kind":"attempt", "outcome":"completed"})
            .await
            .unwrap(),
        1,
    );
    let migrated = index_documents(&db, CONVERSATIONS).await;
    assert!(conversation_indexes
        .iter()
        .all(|index| migrated.contains(index)));
    assert_eq!(index_documents(&db, ACTIVITY).await, activity_indexes);
    assert_eq!(retained_rows(&db).await, rows);
    db.run_command(doc! {"dropUser":RUNTIME_USER})
        .await
        .unwrap();
    db.drop().await.unwrap();
}
