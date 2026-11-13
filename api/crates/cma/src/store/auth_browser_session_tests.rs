//! Real Mongo equivalence and command-budget contracts for joined browser reads.
use std::sync::{Arc, Mutex};

use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
};

use super::*;

struct Fixture {
    client: Client,
    database: String,
    store: Arc<MongoAuthStore>,
    commands: Arc<Mutex<Vec<String>>>,
    measurements: Arc<Mutex<Measurements>>,
}

#[derive(Default)]
struct Measurements {
    reply_bytes: usize,
    command_us: u128,
    checkout_us: u128,
    checkouts: usize,
}

impl Fixture {
    async fn new() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable Mongo replica set");
        let commands = Arc::new(Mutex::new(Vec::new()));
        let observed = commands.clone();
        let measurements = Arc::new(Mutex::new(Measurements::default()));
        let completed = measurements.clone();
        let checked_out = measurements.clone();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.command_event_handler = Some(EventHandler::callback(move |event| match event {
            CommandEvent::Started(event) => observed.lock().unwrap().push(event.command_name),
            CommandEvent::Succeeded(event) => {
                let mut stats = completed.lock().unwrap();
                stats.command_us += event.duration.as_micros();
                stats.reply_bytes += mongodb::bson::serialize_to_vec(&event.reply).unwrap().len();
            }
            _ => {}
        }));
        options.cmap_event_handler = Some(EventHandler::callback(move |event| {
            if let mongodb::event::cmap::CmapEvent::ConnectionCheckedOut(event) = event {
                let mut stats = checked_out.lock().unwrap();
                stats.checkout_us += event.duration.as_micros();
                stats.checkouts += 1;
            }
        }));
        let client = Client::with_options(options).unwrap();
        let database = format!("cma_browser_session_{}", uuid::Uuid::new_v4().simple());
        let key =
            AuthEncryptionKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
        let store = Arc::new(
            MongoAuthStore::open(&client, &database, "browser-session-test", &key)
                .await
                .unwrap(),
        );
        Self {
            client,
            database,
            store,
            commands,
            measurements,
        }
    }

    fn take_commands(&self) -> Vec<String> {
        std::mem::take(&mut *self.commands.lock().unwrap())
    }

    async fn old_read(
        &self,
        id: &SecretDigest,
    ) -> Result<Option<(BrowserLogin, Option<SubjectAuthState>)>, StoreError> {
        let Some(login) = self.store.browser_login(id).await? else {
            return Ok(None);
        };
        let state = if login.expires_at > SystemTime::now() {
            self.store.subject(&login.subject).await?
        } else {
            None
        };
        Ok(Some((login, state)))
    }

    async fn close(self) {
        self.client.database(&self.database).drop().await.unwrap();
    }
}

fn login() -> BrowserLogin {
    BrowserLogin {
        id: SecretDigest::identify("browser-session-test", "session"),
        subject: SubjectId::new("fixture-owner"),
        expires_at: SystemTime::now() + Duration::from_secs(3600),
    }
}

fn state() -> SubjectAuthState {
    SubjectAuthState::new(
        login().subject,
        Authorization::none(),
        Profile::default(),
        Some(DurableSubjectTokens {
            access_token: SecretValue::new("fixture-browser-access".into()),
            refresh_token: Some(SecretValue::new("fixture-browser-refresh".into())),
            expires_at: SystemTime::now() + Duration::from_secs(3600),
        }),
    )
}

fn assert_same(
    left: Result<Option<(BrowserLogin, Option<SubjectAuthState>)>, StoreError>,
    right: Result<Option<(BrowserLogin, Option<SubjectAuthState>)>, StoreError>,
) {
    match (left, right) {
        (Ok(None), Ok(None)) => {}
        (Ok(Some((a, sa))), Ok(Some((b, sb)))) => {
            assert_eq!(
                (a.id, a.subject, a.expires_at),
                (b.id, b.subject, b.expires_at)
            );
            match (sa, sb) {
                (None, None) => {}
                (Some(a), Some(b)) => {
                    assert_eq!(a.subject, b.subject);
                    assert_eq!(a.incarnation, b.incarnation);
                    assert_eq!(a.revision, b.revision);
                    assert_eq!(a.token_generation, b.token_generation);
                    assert_eq!(a.refresh_lease, b.refresh_lease);
                    assert_eq!(a.authorization, b.authorization);
                    assert_eq!(a.profile, b.profile);
                    assert_eq!(a.tokens, b.tokens);
                }
                _ => panic!("subject presence differs"),
            }
        }
        (Err(StoreError::Malformed(_)), Err(StoreError::Malformed(_))) => {}
        (Err(StoreError::Unavailable(_)), Err(StoreError::Unavailable(_))) => {}
        _ => panic!("joined and point readers differ"),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn joined_browser_read_matches_point_reads_with_one_command() {
    let fixture = Fixture::new().await;
    let login = login();
    let row = fixture.store.subject_to_doc(state()).unwrap();
    fixture
        .store
        .subjects
        .insert_one(row.clone())
        .await
        .unwrap();
    fixture
        .store
        .put_browser_login(login.clone())
        .await
        .unwrap();
    fixture.take_commands();
    let old = fixture.old_read(&login.id).await;
    assert_eq!(fixture.take_commands(), ["find", "find"]);
    let joined = fixture.store.browser_session(&login.id).await;
    assert_eq!(fixture.take_commands(), ["aggregate"]);
    assert_same(old, joined);
    fixture.store.subjects.delete_many(doc! {}).await.unwrap();
    assert_same(
        fixture.old_read(&login.id).await,
        fixture.store.browser_session(&login.id).await,
    );
    fixture
        .store
        .browser_logins
        .update_one(doc! {}, doc! {"$set":{"revoked":true}})
        .await
        .unwrap();
    assert_same(
        fixture.old_read(&login.id).await,
        fixture.store.browser_session(&login.id).await,
    );
    assert!(fixture
        .store
        .browser_session(&login.id)
        .await
        .unwrap()
        .is_none());
    fixture
        .store
        .browser_logins
        .delete_many(doc! {})
        .await
        .unwrap();
    assert_same(
        fixture.old_read(&login.id).await,
        fixture.store.browser_session(&login.id).await,
    );
    fixture.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn joined_browser_read_preserves_schema_and_corruption_refusals() {
    let fixture = Fixture::new().await;
    let login = login();
    fixture
        .store
        .put_browser_login(login.clone())
        .await
        .unwrap();
    let subjects = fixture.store.subjects.clone_with_type::<Document>();
    let original =
        mongodb::bson::serialize_to_document(&fixture.store.subject_to_doc(state()).unwrap())
            .unwrap();
    for field in [
        "document_schema_version",
        "incarnation",
        "revision",
        "refresh_lease",
        "token_expires_at",
        "access_token",
    ] {
        let mut corrupt = original.clone();
        corrupt.insert(field, false);
        subjects.delete_many(doc! {}).await.unwrap();
        subjects.insert_one(corrupt).await.unwrap();
        let old = fixture.old_read(&login.id).await;
        assert!(matches!(old, Err(StoreError::Malformed(_))));
        assert_same(old, fixture.store.browser_session(&login.id).await);
    }
    let mut future = original.clone();
    future.insert("document_schema_version", DOCUMENT_SCHEMA_VERSION + 1);
    subjects.delete_many(doc! {}).await.unwrap();
    subjects.insert_one(future).await.unwrap();
    assert_same(
        fixture.old_read(&login.id).await,
        fixture.store.browser_session(&login.id).await,
    );
    // Expired custody must be rejected before the damaged subject is interpreted.
    fixture
        .store
        .browser_logins
        .update_one(
            doc! {},
            doc! {"$set":{"expires_at":DateTime::from_millis(1)}},
        )
        .await
        .unwrap();
    assert_same(
        fixture.old_read(&login.id).await,
        fixture.store.browser_session(&login.id).await,
    );
    fixture
        .store
        .browser_logins
        .delete_many(doc! {})
        .await
        .unwrap();
    fixture
        .store
        .put_browser_login(login.clone())
        .await
        .unwrap();
    fixture
        .store
        .browser_logins
        .update_one(
            doc! {},
            doc! {"$set":{"document_schema_version": DOCUMENT_SCHEMA_VERSION + 1}},
        )
        .await
        .unwrap();
    assert_same(
        fixture.old_read(&login.id).await,
        fixture.store.browser_session(&login.id).await,
    );
    assert!(matches!(
        fixture.store.browser_session(&login.id).await,
        Err(StoreError::Malformed(_))
    ));
    let browsers = fixture.store.browser_logins.clone_with_type::<Document>();
    browsers
        .update_one(doc! {}, doc! {"$unset":{"document_schema_version":""}})
        .await
        .unwrap();
    let old = fixture.old_read(&login.id).await;
    assert!(matches!(old, Err(StoreError::Unavailable(_))));
    assert_same(old, fixture.store.browser_session(&login.id).await);
    fixture.close().await;
}

#[path = "auth_browser_session_request_tests.rs"]
mod requests;

#[tokio::test]
async fn joined_browser_read_refuses_an_unavailable_store() {
    let mut options = ClientOptions::parse("mongodb://127.0.0.1:9/?directConnection=true")
        .await
        .unwrap();
    options.server_selection_timeout = Some(Duration::from_millis(50));
    let client = Client::with_options(options).unwrap();
    let mut store = super::super::tests::store().await;
    store.browser_logins = client
        .database("unavailable_fixture")
        .collection(BROWSER_LOGINS);
    assert!(matches!(
        store.browser_login(&login().id).await,
        Err(StoreError::Unavailable(_))
    ));
    assert!(matches!(
        store.browser_session(&login().id).await,
        Err(StoreError::Unavailable(_))
    ));
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn joined_browser_read_uses_both_primary_key_indexes_without_scans() {
    let fixture = Fixture::new().await;
    let login = login();
    fixture
        .store
        .put_browser_login(login.clone())
        .await
        .unwrap();
    fixture.store.insert_subject(state()).await.unwrap();
    let explain = fixture.client.database(&fixture.database).run_command(doc! {
        "explain": {"aggregate": BROWSER_LOGINS, "pipeline": pipeline(&login.id).to_vec(), "cursor": {}},
        "verbosity": "executionStats"
    }).await.unwrap();
    let stages = explain.get_array("stages").unwrap();
    let cursor = stages
        .iter()
        .filter_map(|stage| stage.as_document())
        .find_map(|stage| stage.get_document("$cursor").ok())
        .unwrap();
    let stats = cursor.get_document("executionStats").unwrap();
    assert_eq!(integer(stats, "totalDocsExamined"), 1);
    assert_eq!(integer(stats, "totalKeysExamined"), 1);
    let lookup = stages
        .iter()
        .filter_map(|stage| stage.as_document())
        .find(|stage| stage.contains_key("$lookup"))
        .unwrap();
    assert_eq!(integer(lookup, "totalDocsExamined"), 1);
    assert_eq!(integer(lookup, "totalKeysExamined"), 1);
    assert_eq!(integer(lookup, "collectionScans"), 0);
    assert_eq!(
        lookup.get_array("indexesUsed").unwrap(),
        &[mongodb::bson::Bson::String("_id_".into())]
    );
    println!("identity_explain baseline_docs=2 baseline_keys=2 baseline_collection_scans=0 outer_docs=1 outer_keys=1 joined_docs=1 joined_keys=1 joined_collection_scans=0 indexes=_id_");
    fixture.close().await;
}

fn integer(document: &Document, field: &str) -> i64 {
    match document.get(field).unwrap() {
        mongodb::bson::Bson::Int32(value) => i64::from(*value),
        mongodb::bson::Bson::Int64(value) => *value,
        _ => panic!("explain counter must be an integer"),
    }
}
