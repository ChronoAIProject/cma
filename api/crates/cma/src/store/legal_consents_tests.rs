use layer2_core::{DirectoryQuery, PageRequest, PublicIdentity, UserDirectoryStore};
use mongodb::{bson::DateTime, Client};

use super::super::directory::MongoUserDirectoryStore;
use super::*;

const FIRST: &str = "2026-11-23T01:00:00Z";
const LATER: &str = "2026-11-23T02:00:00Z";

async fn database() -> (Client, Database) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable Mongo URI");
    let client = Client::with_uri_str(uri).await.unwrap();
    let database = client.database(&format!("cma_legal_{}", uuid::Uuid::new_v4().simple()));
    (client, database)
}

fn identity(subject: &str, name: &str) -> PublicIdentity {
    PublicIdentity {
        subject: SubjectId::new(subject),
        display_name: Some(name.into()),
        avatar_url: None,
    }
}

#[test]
fn consent_rows_tolerate_fields_added_later() {
    let row: ConsentRow = mongodb::bson::deserialize_from_document(doc! {
        "_id": "owner",
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "display_name": "Owner",
        "observed_at": DateTime::now(),
    })
    .unwrap();
    assert!(row.legal_consents.is_empty());
    let later: ConsentRow = mongodb::bson::deserialize_from_document(doc! {
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        FIELD: [{ "version": "1", "accepted_at": FIRST, "added_later": true }],
    })
    .unwrap();
    assert_eq!(later.legal_consents[0].version, "1");
}

#[tokio::test]
#[ignore = "requires disposable Mongo fixture"]
async fn acceptance_is_recorded_once_and_survives_sign_in_observation() {
    let (_client, database) = database().await;
    let client = database.client().clone();
    let directory = MongoUserDirectoryStore::open(&client, database.name())
        .await
        .unwrap();
    let store = MongoLegalConsentStore::new(&database);
    let owner = SubjectId::new("legal-owner");
    directory
        .observe(&identity("legal-owner", "Legal Owner"))
        .await
        .unwrap();

    assert_eq!(store.acceptance(&owner, "1").await.unwrap(), None);
    let first = store.accept(&owner, "1", FIRST).await.unwrap();
    assert_eq!(first.accepted_at, FIRST);
    let replay = store.accept(&owner, "1", LATER).await.unwrap();
    assert_eq!(replay, first);

    directory
        .observe(&identity("legal-owner", "Renamed Owner"))
        .await
        .unwrap();
    assert_eq!(store.acceptance(&owner, "1").await.unwrap(), Some(first));
    let user = directory.user(&owner).await.unwrap().unwrap();
    assert_eq!(user.identity.display_name.as_deref(), Some("Renamed Owner"));

    store.accept(&owner, "2", LATER).await.unwrap();
    assert!(store.acceptance(&owner, "1").await.unwrap().is_some());
    assert_eq!(
        store
            .acceptance(&owner, "2")
            .await
            .unwrap()
            .unwrap()
            .accepted_at,
        LATER
    );
    assert_eq!(
        store
            .acceptance(&SubjectId::new("other-owner"), "1")
            .await
            .unwrap(),
        None
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo fixture"]
async fn consent_never_creates_registration_evidence() {
    let (client, database) = database().await;
    let directory = MongoUserDirectoryStore::open(&client, database.name())
        .await
        .unwrap();
    let store = MongoLegalConsentStore::new(&database);
    let owner = SubjectId::new("unregistered-owner");
    let users = database.collection::<Document>(USERS);

    assert!(matches!(
        store.accept(&owner, "1", FIRST).await,
        Err(StoreError::ReferenceUnavailable(_))
    ));
    assert_eq!(users.count_documents(doc! {}).await.unwrap(), 0);

    directory
        .observe(&identity("unregistered-owner", "Now Registered"))
        .await
        .unwrap();
    let (left, right) = tokio::join!(
        store.accept(&owner, "1", FIRST),
        store.accept(&owner, "1", LATER)
    );
    let (left, right) = (left.unwrap(), right.unwrap());
    assert_eq!(left, right);
    let stored = users
        .find_one(doc! { "_id": "unregistered-owner" })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.get_array(FIELD).unwrap().len(), 1);
    let listed = directory
        .list(&DirectoryQuery {
            page: PageRequest::default(),
            text: None,
        })
        .await
        .unwrap();
    assert_eq!(listed.items.len(), 1);
    assert_eq!(
        listed.items[0].identity.display_name.as_deref(),
        Some("Now Registered")
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo fixture"]
async fn unreadable_or_future_rows_are_refused_and_never_written() {
    let (_client, database) = database().await;
    let users = database.collection::<Document>(USERS);
    let store = MongoLegalConsentStore::new(&database);
    users
        .insert_many([
            doc! { "_id": "future", "document_schema_version": DOCUMENT_SCHEMA_VERSION + 1 },
            doc! {
                "_id": "corrupt",
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                FIELD: [{ "version": "has space", "accepted_at": FIRST }],
            },
            doc! {
                "_id": "untimed",
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                FIELD: [{ "version": "1", "accepted_at": "yesterday" }],
            },
        ])
        .await
        .unwrap();
    for subject in ["future", "corrupt", "untimed"] {
        let subject = SubjectId::new(subject);
        assert!(matches!(
            store.acceptance(&subject, "1").await,
            Err(StoreError::Malformed(_))
        ));
    }
    assert!(matches!(
        store.accept(&SubjectId::new("future"), "1", FIRST).await,
        Err(StoreError::Malformed(_))
    ));
    let future = users
        .find_one(doc! { "_id": "future" })
        .await
        .unwrap()
        .unwrap();
    assert!(!future.contains_key(FIELD));
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo fixture"]
async fn retained_history_is_bounded() {
    let (client, database) = database().await;
    MongoUserDirectoryStore::open(&client, database.name())
        .await
        .unwrap()
        .observe(&identity("history-owner", "History Owner"))
        .await
        .unwrap();
    let store = MongoLegalConsentStore::new(&database);
    let owner = SubjectId::new("history-owner");
    for index in 0..=MAX_RETAINED_ACCEPTANCES {
        store
            .accept(&owner, &format!("v{index}"), FIRST)
            .await
            .unwrap();
    }
    assert_eq!(store.acceptance(&owner, "v0").await.unwrap(), None);
    assert!(store
        .acceptance(&owner, &format!("v{MAX_RETAINED_ACCEPTANCES}"))
        .await
        .unwrap()
        .is_some());
    database.drop().await.unwrap();
}
