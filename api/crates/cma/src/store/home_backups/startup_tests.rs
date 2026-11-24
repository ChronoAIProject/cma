use super::*;
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
};
use std::sync::{Arc, Mutex};

async fn disabled_startup(retained: bool) {
    assert!(!crate::config::FeatureFlags::default().home_backup_enabled);
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("home_backup_startup_{}", uuid::Uuid::new_v4().simple());
    let db = admin.database(&name);
    let rows = [
        (
            SCHEDULES_COLLECTION,
            schedule_document(&schedule()).unwrap(),
        ),
        (
            BACKUPS_COLLECTION,
            backup_document(&standalone_backup(HomeBackupState::Creating)).unwrap(),
        ),
    ];
    if retained {
        for (collection, row) in &rows {
            db.collection(collection)
                .insert_one(row.clone())
                .await
                .unwrap();
        }
    }
    let commands = Arc::new(Mutex::new(Vec::new()));
    let observed = commands.clone();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            observed.lock().unwrap().push((event.db, event.command));
        }
    }));
    let client = Client::with_options(options).unwrap();
    let key = crate::store::AuthEncryptionKey::from_base64(
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
    )
    .unwrap();
    let result = crate::store::MongoStores::open(
        client,
        &name,
        "backup-fixture",
        &key,
        Duration::from_secs(90 * 86400),
        Arc::new(crate::store::checkpoint_storage::RetiredCheckpointStorage),
        ("runtime-fixture", None),
    )
    .await;
    let backup_commands: Vec<_> = commands
        .lock()
        .unwrap()
        .iter()
        .filter(|(database, command)| {
            database == &name
                && rows
                    .iter()
                    .any(|(collection, _)| command.to_string().contains(collection))
        })
        .cloned()
        .collect();
    let names = db.list_collection_names().await.unwrap();
    let mut unchanged = true;
    for (collection, row) in &rows {
        unchanged &= if retained {
            db.collection::<Document>(collection)
                .find_one(doc! {})
                .await
                .unwrap()
                == Some(row.clone())
                && db
                    .collection::<Document>(collection)
                    .list_indexes()
                    .await
                    .unwrap()
                    .try_collect::<Vec<_>>()
                    .await
                    .unwrap()
                    .len()
                    == 1
        } else {
            !names.iter().any(|name| name == collection)
        };
    }
    let succeeded = result.is_ok();
    let error = result.err();
    db.drop().await.unwrap();
    assert!(
        succeeded,
        "disabled production constructor failed: {error:?}"
    );
    assert!(
        backup_commands.is_empty(),
        "disabled backup startup sent commands: {backup_commands:?}"
    );
    assert!(
        unchanged,
        "disabled startup changed the backup schema or retained work"
    );
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn disabled_production_startup_leaves_absent_backup_schema_untouched() {
    disabled_startup(false).await;
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn disabled_production_startup_neither_reads_nor_changes_retained_backup_work() {
    disabled_startup(true).await;
}

fn existing_role(database: &str) -> Document {
    let mut role = crate::store::isolation::role(database);
    role.get_array_mut("privileges").unwrap().retain(|grant| {
        let collection = grant
            .as_document()
            .unwrap()
            .get_document("resource")
            .unwrap()
            .get_str("collection")
            .unwrap();
        ![SCHEDULES_COLLECTION, BACKUPS_COLLECTION].contains(&collection)
    });
    role
}

#[test]
fn existing_runtime_grants_do_not_require_backup_access_but_partial_grants_are_refused() {
    let role = existing_role("fixture");
    let grants = role.get_array("privileges").unwrap();
    let status = |grants: Vec<mongodb::bson::Bson>| {
        doc! {"authInfo":{
        "authenticatedUsers":[{"user":"runtime-fixture"}], "authenticatedUserPrivileges":grants}}
    };
    crate::store::isolation::validate(&status(grants.clone()), "fixture").unwrap();
    for collection in [SCHEDULES_COLLECTION, BACKUPS_COLLECTION] {
        let mut partial = grants.clone();
        partial.push(
            doc! {"resource":{"db":"fixture","collection":collection},"actions":["find"]}.into(),
        );
        assert!(crate::store::isolation::validate(&status(partial), "fixture").is_err());
    }
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn production_connection_accepts_existing_credentials_without_backup_grants_or_schema() {
    use crate::store::mongo::MongoConnectionPolicy;
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("home_backup_authority_{}", uuid::Uuid::new_v4().simple());
    let db = admin.database(&name);
    db.run_command(existing_role(&name)).await.unwrap();
    let password = uuid::Uuid::new_v4().simple().to_string();
    db.run_command(doc! {"createUser":"backup-runtime","pwd":&password,
    "roles":[{"role":"cma_am_runtime","db":&name}]})
        .await
        .unwrap();
    let mut runtime_uri = url::Url::parse(&uri).unwrap();
    runtime_uri.set_username("backup-runtime").unwrap();
    runtime_uri.set_password(Some(&password)).unwrap();
    runtime_uri
        .query_pairs_mut()
        .append_pair("authSource", &name);
    let key = crate::store::AuthEncryptionKey::from_base64(
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
    )
    .unwrap();
    let result = crate::store::MongoStores::connect_with_policy(
        runtime_uri.as_str(),
        &name,
        "backup-fixture",
        &key,
        Duration::from_secs(90 * 86400),
        ("runtime-fixture", None),
        MongoConnectionPolicy {
            authority: cma_service_auth::mongodb_authority::MongoAuthorityMode::Isolated,
            agent_manager_replicas: false,
            shared_observation: false,
            startup_schema: crate::config::StartupSchemaMode::Observe,
            initial_settings: Default::default(),
            released: None,
            runtime_pointer_required: false,
        },
    )
    .await;
    let names = db.list_collection_names().await.unwrap();
    let error = result.err();
    db.run_command(doc! {"dropUser":"backup-runtime"})
        .await
        .unwrap();
    db.drop().await.unwrap();
    assert!(
        error.is_none(),
        "existing runtime credentials refused: {error:?}"
    );
    assert!(!names
        .iter()
        .any(|name| [SCHEDULES_COLLECTION, BACKUPS_COLLECTION].contains(&name.as_str())));
}
