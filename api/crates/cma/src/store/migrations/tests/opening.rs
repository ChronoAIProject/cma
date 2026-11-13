use super::*;
use crate::store::migrations::{opening_index, steps::Step};
use mongodb::bson::Bson;

#[test]
fn opening_index_require_complete_ordered_unfiltered_keys() {
    let mut total = 0;
    for collection in opening_index::COLLECTIONS {
        let indexes = opening_index::indexes(collection);
        total += indexes.len();
        assert!(opening_index::missing(collection, &indexes)
            .unwrap()
            .is_empty());
        assert_eq!(
            opening_index::missing(collection, &[]).unwrap().len(),
            indexes.len()
        );
        for (key, value) in [
            ("unique", Bson::Boolean(true)),
            ("sparse", Bson::Boolean(true)),
            ("hidden", Bson::Boolean(true)),
            ("expireAfterSeconds", Bson::Int64(1)),
            ("partialFilterExpression", doc! {"live":true}.into()),
            ("collation", doc! {"locale":"en"}.into()),
        ] {
            let mut drift = indexes.clone();
            drift[0].insert(key, value);
            assert!(opening_index::missing(collection, &drift).is_err());
        }
        let mut drift = indexes.clone();
        let keys = drift[0].get_document("key").unwrap();
        let reversed: Document = keys
            .iter()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        drift[0].insert("key", reversed);
        assert!(opening_index::missing(collection, &drift).is_err());
        let mut duplicate = indexes.clone();
        duplicate.push(indexes[0].clone());
        assert!(opening_index::missing(collection, &duplicate).is_err());
    }
    assert_eq!(total, 1);
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn opening_index_migration_is_explicit_repeatable_and_separately_receipted() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let database = client.database(&format!(
        "cma_opening_index_{}",
        uuid::Uuid::new_v4().simple()
    ));
    assert!(Step::AgentOpening.inspect(&database).await.is_err());
    assert!(database.list_collection_names().await.unwrap().is_empty());
    for name in opening_index::COLLECTIONS {
        database.create_collection(name).await.unwrap();
    }
    assert_eq!(
        Step::AgentOpening.inspect(&database).await.unwrap().len(),
        1
    );
    let collection = ledger::Ledger::for_step(client.clone(), database.clone(), Step::AgentOpening);
    let first = apply_indexes(&collection, &database, &identity())
        .await
        .unwrap();
    assert_eq!(first["created_indexes"], 1);
    assert_eq!(
        apply_indexes(&collection, &database, &identity())
            .await
            .unwrap()["outcome"],
        "already_completed"
    );
    assert!(collection.completed().await.unwrap());
    let legacy = ledger::Ledger::new(client.clone(), database.clone());
    legacy.preflight().await.unwrap();
    assert!(!legacy.completed().await.unwrap());
    assert_eq!(
        legacy.status().await.unwrap()["recent_attempts"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    apply_indexes(&legacy, &database, &identity())
        .await
        .unwrap();
    assert!(legacy.completed().await.unwrap());
    assert_eq!(
        collection.status().await.unwrap()["recent_attempts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    database
        .collection::<Document>("agent_log")
        .drop_index("agent_log_kind_sequence")
        .await
        .unwrap();
    assert!(apply_indexes(&collection, &database, &identity())
        .await
        .is_err());
    database.drop().await.unwrap();
}
