use super::*;
use crate::store::sandbox_mongo::MongoSandboxStore;
use mongodb::Client;

fn index() -> Document {
    doc! {"v":2,"name":NAME,"key":keys()}
}

#[test]
fn interval_index_accepts_only_complete_equivalent_lookup() {
    assert!(!present(&[doc! {"name":"_id_","key":{"_id":1}}]).unwrap());
    assert!(present(&[index()]).unwrap());
    let mut equivalent = index();
    equivalent.insert("name", "existing_interval_lookup");
    for option in ["unique", "sparse", "hidden"] {
        equivalent.insert(option, false);
    }
    assert!(present(&[equivalent]).unwrap());
    for (option, value) in [
        ("unique", Bson::Boolean(true)),
        ("sparse", true.into()),
        ("hidden", true.into()),
        (
            "partialFilterExpression",
            doc! {"sandbox_id":{"$exists":true}}.into(),
        ),
        ("expireAfterSeconds", 600_i32.into()),
        ("collation", doc! {"locale":"en"}.into()),
        ("buildUUID", "unfinished".into()),
    ] {
        let mut drift = index();
        drift.insert(option, value);
        assert!(present(&[drift]).is_err(), "{option}");
    }
}

#[test]
fn interval_index_refuses_ambiguous_keys_names_and_default_collation() {
    let mut wrong = index();
    wrong.insert("key", doc! {"sandbox_id":1,"revision":1});
    assert!(present(&[wrong]).is_err());
    let mut reordered = index();
    reordered.insert(
        "key",
        doc! {"ended_at":1,"sandbox_id":1,"started_at":1,"revision":1},
    );
    assert!(present(&[reordered]).is_err());
    assert!(present(&[index(), index()]).is_err());
    assert!(present(&[doc! {"spec":index(),"buildUUID":"unfinished"}]).is_err());
    assert!(present(&[doc! {"name":"_id_","key":{"_id":1},"collation":{"locale":"en"}}]).is_err());
}

async fn fixture() -> (Client, Database) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
    let client = Client::with_uri_str(uri).await.unwrap();
    let name = std::env::var("CHRONO_TEST_ALLOCATION_DB_01")
        .unwrap_or_else(|_| format!("usage_interval_{}", uuid::Uuid::new_v4().simple()));
    let database = client.database(&name);
    (client, database)
}

async fn inventory(database: &Database) -> Vec<Document> {
    let response = database
        .run_command(doc! {
            "listIndexes":super::super::EVENTS,"cursor":{"batchSize":64},"maxTimeMS":2000,
        })
        .await
        .unwrap();
    response
        .get_document("cursor")
        .unwrap()
        .get_array("firstBatch")
        .unwrap()
        .iter()
        .map(|value| value.as_document().unwrap().clone())
        .collect()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn interval_index_real_startup_creates_and_reopens_without_replacing_existing_indexes() {
    let (client, database) = fixture().await;
    MongoSandboxStore::open(&client, database.name())
        .await
        .unwrap();
    let first = inventory(&database).await;
    assert!(present(&first).unwrap());
    let desired = first
        .iter()
        .find(|index| index.get_str("name").ok() == Some(NAME))
        .unwrap();
    assert!(desired
        .get_document("key")
        .unwrap()
        .iter()
        .eq(keys().iter()));
    assert_eq!(first.len(), 5, "_id, revision, owner, billing and interval");
    MongoSandboxStore::open(&client, database.name())
        .await
        .unwrap();
    assert_eq!(first, inventory(&database).await);
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn interval_index_resumes_partial_startup_and_coalesces_concurrent_ensure() {
    let (client, database) = fixture().await;
    MongoSandboxStore::open(&client, database.name())
        .await
        .unwrap();
    let events = database.collection::<Document>(super::super::EVENTS);
    events.drop_index(NAME).await.unwrap();
    // Existing startup indexes can be durable before this additive step starts.
    let (left, right) = tokio::join!(ensure(&database), ensure(&database));
    // An observed in-progress build may refuse this startup; it cannot be
    // mistaken for a ready index. The durable completed result is retryable.
    assert!(left.is_ok() || right.is_ok());
    assert_eq!(inventory(&database).await.len(), 5);
    MongoSandboxStore::open(&client, database.name())
        .await
        .unwrap();
    assert!(inspect(&database).await.unwrap());
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn interval_index_real_startup_accepts_equivalent_existing_name() {
    let (client, database) = fixture().await;
    MongoSandboxStore::open(&client, database.name())
        .await
        .unwrap();
    database
        .collection::<Document>(super::super::EVENTS)
        .drop_index(NAME)
        .await
        .unwrap();
    database
        .run_command(doc! {"createIndexes":super::super::EVENTS,
        "indexes":[{"name":"existing_interval_lookup","key":keys()}]})
        .await
        .unwrap();
    let before = inventory(&database).await;
    MongoSandboxStore::open(&client, database.name())
        .await
        .unwrap();
    assert_eq!(before, inventory(&database).await);
    assert!(!before
        .iter()
        .any(|index| index.get_str("name").ok() == Some(NAME)));
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn interval_index_real_startup_refuses_conflicting_options_without_repair() {
    let (client, database) = fixture().await;
    MongoSandboxStore::open(&client, database.name())
        .await
        .unwrap();
    let events = database.collection::<Document>(super::super::EVENTS);
    for (option, value) in [
        ("unique", Bson::Boolean(true)),
        ("sparse", true.into()),
        ("hidden", true.into()),
        (
            "partialFilterExpression",
            doc! {"sandbox_id":{"$exists":true}}.into(),
        ),
        ("collation", doc! {"locale":"en"}.into()),
    ] {
        events.drop_index(NAME).await.unwrap();
        let mut drift = doc! {"name":NAME,"key":keys()};
        drift.insert(option, value);
        database
            .run_command(doc! {"createIndexes":super::super::EVENTS,"indexes":[drift]})
            .await
            .unwrap();
        let before = inventory(&database).await;
        assert!(
            MongoSandboxStore::open(&client, database.name())
                .await
                .is_err(),
            "{option}"
        );
        assert_eq!(
            before,
            inventory(&database).await,
            "must not silently repair {option}"
        );
    }
    database.drop().await.unwrap();
}
