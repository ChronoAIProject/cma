use super::*;
use crate::auth::{
    persistence::{AuthStore, BrowserLogin, SecretDigest, SubjectAuthState},
    profile::Profile,
};
use layer2_core::{
    sandbox::{HierarchyReadService, OwnerSandboxQuery},
    Authorization, Caller, Capability,
};
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

async fn fixture() -> (Client, Database, Arc<Mutex<Vec<String>>>) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("owned disposable local replica set");
    assert!(uri.starts_with("mongodb://127.0.0.1:") || uri.starts_with("mongodb://localhost:"));
    let commands = Arc::new(Mutex::new(Vec::new()));
    let captured = commands.clone();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            captured.lock().unwrap().push(event.command_name);
        }
    }));
    let client = Client::with_options(options).unwrap();
    let db = client.database(&format!("cma_inert_read_{}", uuid::Uuid::new_v4().simple()));
    (client, db, commands)
}

#[tokio::test]
#[ignore = "requires an owned loopback CHRONO_TEST_MONGODB_URI replica set"]
async fn missing_read_collections_refuse_without_startup_ddl() {
    let (client, db, commands) = fixture().await;
    assert!(ReadOnlyHierarchyStore::open(client.clone(), db.clone())
        .await
        .is_err());
    let key = crate::store::AuthEncryptionKey::from_base64(
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
    )
    .unwrap();
    assert!(
        crate::store::auth::read_only::open(&client, db.name(), "test", &key)
            .await
            .is_err()
    );
    assert!(db.list_collection_names().await.unwrap().is_empty());
    assert!(commands
        .lock()
        .unwrap()
        .iter()
        .all(|name| name != "create" && name != "createIndexes"));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned loopback CHRONO_TEST_MONGODB_URI replica set"]
async fn actual_inert_store_reads_and_refusals_issue_only_read_commands() {
    let (client, db, commands) = fixture().await;
    for name in [SANDBOXES, WORKSPACES, WORKTREES, AGENTS, RESOURCE_POLICIES] {
        db.create_collection(name).await.unwrap();
    }
    let mut own = crate::store::sandbox_mongo_integration_tests::sandbox("sbx_inert_own");
    own.name = "own".into();
    let mut foreign = own.clone();
    foreign.id = SandboxId("sbx_inert_foreign".into());
    foreign.name = "foreign".into();
    foreign.owner = SubjectId::new("another-owner");
    for sandbox in [&own, &foreign] {
        db.collection::<Document>(SANDBOXES)
            .insert_one(crate::store::sandboxes::sandbox_document(sandbox, 0).unwrap())
            .await
            .unwrap();
    }
    let key = crate::store::AuthEncryptionKey::from_base64(
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
    )
    .unwrap();
    let fixture_auth = crate::store::MongoAuthStore::open(&client, db.name(), "test", &key)
        .await
        .unwrap();
    let state = SubjectAuthState::new(
        own.owner.clone(),
        Authorization::none(),
        Profile::default(),
        None,
    );
    fixture_auth.insert_subject(state.clone()).await.unwrap();
    let login = SecretDigest::identify("inert-fixture", "one");
    fixture_auth
        .put_browser_login(BrowserLogin {
            id: login.clone(),
            subject: own.owner.clone(),
            expires_at: SystemTime::now() + Duration::from_secs(3600),
        })
        .await
        .unwrap();
    commands.lock().unwrap().clear();
    let store = Arc::new(
        ReadOnlyHierarchyStore::open(client.clone(), db.clone())
            .await
            .unwrap(),
    );
    let service = HierarchyReadService::new(store.clone());
    let caller = Caller::new(
        own.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let inventory = service
        .list_sandboxes(&caller, &OwnerSandboxQuery::default())
        .await
        .unwrap();
    assert_eq!(inventory.items.len(), 1);
    assert_eq!(inventory.items[0].sandbox, own);
    assert_eq!(
        service.sandbox(&caller, &own.id).await.unwrap().sandbox,
        own
    );
    assert!(matches!(
        service.sandbox(&caller, &foreign.id).await,
        Err(layer2_core::sandbox::HierarchyReadError::NotFound)
    ));
    let auth = crate::store::auth::read_only::open(&client, db.name(), "test", &key)
        .await
        .unwrap();
    assert_eq!(
        auth.browser_session(&login)
            .await
            .unwrap()
            .unwrap()
            .0
            .subject,
        own.owner
    );
    assert!(auth.delete_browser_login(&login).await.is_err());
    assert!(auth.insert_subject(state.clone()).await.is_err());
    assert!(auth
        .replace_subject(&state.incarnation, state.revision, state.clone())
        .await
        .is_err());
    assert!(auth.browser_login(&login).await.unwrap().is_some());
    let captured = commands.lock().unwrap().clone();
    assert!(captured.iter().any(|name| name == "aggregate"));
    assert!(
        captured.iter().all(|name| matches!(
            name.as_str(),
            "find"
                | "aggregate"
                | "getMore"
                | "killCursors"
                | "listCollections"
                | "hello"
                | "endSessions"
                | "ping"
        )),
        "unexpected command kinds: {captured:?}"
    );
    db.drop().await.unwrap();
}
