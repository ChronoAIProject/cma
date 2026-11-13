use super::*;
use crate::store::migrations::{collection_indexes, steps::Step};
use mongodb::bson::Bson;

#[test]
fn collection_indexes_require_complete_ordered_unfiltered_keys() {
    let mut total = 0;
    for collection in collection_indexes::COLLECTIONS {
        let indexes = collection_indexes::indexes(collection);
        total += indexes.len();
        assert!(collection_indexes::missing(collection, &indexes)
            .unwrap()
            .is_empty());
        assert_eq!(
            collection_indexes::missing(collection, &[]).unwrap().len(),
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
            assert!(collection_indexes::missing(collection, &drift).is_err());
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
        assert!(collection_indexes::missing(collection, &drift).is_err());
        let mut duplicate = indexes.clone();
        duplicate.push(indexes[0].clone());
        assert!(collection_indexes::missing(collection, &duplicate).is_err());
    }
    assert_eq!(total, 5);
}

#[test]
fn manifest_selection_is_explicit_and_preserves_legacy_cli() {
    let args = |values: &[&str]| values.iter().map(|v| v.to_string()).collect::<Vec<_>>();
    assert_eq!(
        invocation(&args(&["--migration-plan"])).unwrap().0,
        Step::Activity
    );
    assert_eq!(
        invocation(&args(&["--migration-plan", collection_indexes::ID]))
            .unwrap()
            .0,
        Step::LedgerCollection
    );
    assert!(invocation(&args(&["--migration-apply", "future", "hash", "image"])).is_err());
    assert!(invocation(&args(&[
        "--migration-plan",
        collection_indexes::ID,
        "extra"
    ]))
    .is_err());
    let role = authority::role_for("fixture", true, Step::LedgerCollection);
    for grant in role.get_array("privileges").unwrap() {
        let grant = grant.as_document().unwrap();
        let collection = grant
            .get_document("resource")
            .unwrap()
            .get_str("collection")
            .unwrap();
        let actions = grant.get_array("actions").unwrap();
        assert!(!actions.contains(&Bson::String("remove".into())));
        if collection != ledger::COLLECTION {
            assert!(!actions.contains(&Bson::String("createCollection".into())));
            assert_eq!(actions.len(), 2);
        }
    }
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn collection_index_migration_is_explicit_repeatable_and_separately_receipted() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let database = client.database(&format!(
        "cma_collection_indexes_{}",
        uuid::Uuid::new_v4().simple()
    ));
    assert!(Step::LedgerCollection.inspect(&database).await.is_err());
    assert!(database.list_collection_names().await.unwrap().is_empty());
    for name in collection_indexes::COLLECTIONS {
        database.create_collection(name).await.unwrap();
    }
    assert_eq!(
        Step::LedgerCollection
            .inspect(&database)
            .await
            .unwrap()
            .len(),
        5
    );
    let collection =
        ledger::Ledger::for_step(client.clone(), database.clone(), Step::LedgerCollection);
    let first = apply_indexes(&collection, &database, &identity())
        .await
        .unwrap();
    assert_eq!(first["created_indexes"], 5);
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
        .collection::<Document>("agents")
        .drop_index("ledger_agents_worktree_id")
        .await
        .unwrap();
    assert!(apply_indexes(&collection, &database, &identity())
        .await
        .is_err());
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn collection_step_shares_global_lease_and_refuses_future_manifest() {
    let client = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let database = client.database(&format!(
        "cma_collection_fence_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let collection =
        ledger::Ledger::for_step(client.clone(), database.clone(), Step::LedgerCollection);
    let legacy = ledger::Ledger::new(client, database.clone());
    collection.initialize().await.unwrap();
    legacy.initialize().await.unwrap();
    let lease = collection.claim(&identity()).await.unwrap();
    assert!(legacy.claim(&identity()).await.is_err());
    collection
        .finish(&lease, Err("fixture refusal"))
        .await
        .unwrap();
    let old = legacy.claim(&identity()).await.unwrap();
    legacy.finish(&old, Err("fixture refusal")).await.unwrap();
    database.collection::<Document>(ledger::COLLECTION).insert_one(doc!{
        "_id":"manifest:future", "schema":1_i64,"kind":"manifest","migration":"future","checksum":"unknown","completed":false
    }).await.unwrap();
    assert!(legacy.preflight().await.is_err());
    assert!(collection.preflight().await.is_err());
    database.drop().await.unwrap();
}
