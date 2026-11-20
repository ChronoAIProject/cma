use super::*;
use crate::store::{
    agent_log_segments::{self as segments, mongo_tests::fixtures},
    sandbox_mongo::MongoSandboxStore,
};
use layer2_core::sandbox::store::{AgentLogStore, SandboxStoreError};
use mongodb::bson::Bson;

#[test]
fn source_key_index_requires_the_exact_partial_filter_and_binary_order() {
    let index = agent_log_source_keys::index();
    assert!(
        index_inventory::strict_missing(std::slice::from_ref(&index), &index)
            .unwrap()
            .is_empty()
    );
    for (key, value) in [
        ("key", doc! {"agent_id":-1}.into()),
        ("partialFilterExpression", doc! {"seq":null}.into()),
        ("unique", true.into()),
        ("sparse", true.into()),
        ("hidden", true.into()),
        ("expireAfterSeconds", 1_i64.into()),
        ("collation", doc! {"locale":"en"}.into()),
        ("name", Bson::String("different".into())),
    ] {
        let mut drift = index.clone();
        drift.insert(key, value);
        assert!(
            index_inventory::strict_missing(&[drift], &index).is_err(),
            "{key}"
        );
    }
    let step = Step::AgentLogSourceKeys;
    assert_eq!(step.max_changes(), 1);
    assert_eq!(step.collections(), &["agent_log"]);
    for apply in [false, true] {
        let role = authority::role_for("synthetic", apply, step);
        for grant in role.get_array("privileges").unwrap() {
            let grant = grant.as_document().unwrap();
            let actions = grant.get_array("actions").unwrap();
            assert!(actions
                .iter()
                .all(|a| !matches!(a.as_str(), Some("remove" | "dropIndex" | "collMod"))));
        }
    }
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn source_key_migration_is_explicit_repeatable_and_compatible_with_serving_startup() {
    let (client, name) = fixtures::database("key_migrate").await;
    let db = client.database(&name);
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    // The unchanged serving startup and legacy index list must leave this
    // explicit migration pending, including when opening the store again.
    assert_eq!(agent_log_source_keys::inspect(&db).await.unwrap().len(), 1);
    MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(agent_log_source_keys::inspect(&db).await.unwrap().len(), 1);
    let source = doc! {"_id":"retained-source","document_schema_version":1_i64,"agent_id":"synthetic","seq":"malformed"};
    store
        .agent_log
        .insert_one(source.clone())
        .bypass_document_validation(true)
        .await
        .unwrap();
    let ledger = ledger::Ledger::for_step(client.clone(), db.clone(), Step::AgentLogSourceKeys);
    assert_eq!(
        apply_indexes(&ledger, &db, &identity()).await.unwrap()["created_indexes"],
        1
    );
    assert_eq!(
        apply_indexes(&ledger, &db, &identity()).await.unwrap()["outcome"],
        "already_completed"
    );
    assert!(ledger.completed().await.unwrap());
    assert!(agent_log_source_keys::inspect(&db)
        .await
        .unwrap()
        .is_empty());
    // Existing serving checks tolerate the additive index and migration
    // receipts; the source representation and old sequence index are retained.
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .agent_log
            .find_one(doc! {"_id":"retained-source"})
            .await
            .unwrap(),
        Some(source)
    );
    assert!(reopened
        .agent_log
        .find_one(doc! {"agent_id":"synthetic","seq":"malformed"})
        .hint(mongodb::options::Hint::Name("agent_log_sequence".into()))
        .await
        .unwrap()
        .is_some());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn absent_or_drifted_source_key_index_refuses_without_ddl_or_fallback() {
    let (client, name) = fixtures::database("key_drift").await;
    let db = client.database(&name);
    segments::schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, entries) = fixtures::segmented_with_width(&store, 3, 2, 2).await;
    store
        .agent_log
        .drop_index(agent_log_source_keys::INDEX)
        .await
        .unwrap();
    for drift in [
        None,
        Some(
            doc! {"name":agent_log_source_keys::INDEX,"key":{"agent_id":1},"partialFilterExpression":{"seq":null}},
        ),
        Some(doc! {"name":agent_log_source_keys::INDEX,"key":{"agent_id":-1}}),
        Some(
            doc! {"name":agent_log_source_keys::INDEX,"key":{"agent_id":1},"collation":{"locale":"en"}},
        ),
    ] {
        if let Some(index) = &drift {
            db.run_command(doc! {"createIndexes":"agent_log","indexes":[index]})
                .await
                .unwrap();
        }
        let before = db
            .run_command(doc! {"listIndexes":"agent_log"})
            .await
            .unwrap();
        let mut visits = 0;
        let result = crate::store::agent_log_store::visit_page(
            &store,
            &agent.id,
            0,
            2,
            2,
            false,
            &mut |_| {
                visits += 1;
                Ok(())
            },
        )
        .await;
        assert!(
            matches!(result, Err(SandboxStoreError::Unavailable(_))),
            "{result:?}"
        );
        assert_eq!(visits, 0);
        let after = db
            .run_command(doc! {"listIndexes":"agent_log"})
            .await
            .unwrap();
        assert_eq!(
            before
                .get_document("cursor")
                .unwrap()
                .get_array("firstBatch")
                .unwrap(),
            after
                .get_document("cursor")
                .unwrap()
                .get_array("firstBatch")
                .unwrap()
        );
        if drift.is_some() {
            store
                .agent_log
                .drop_index(agent_log_source_keys::INDEX)
                .await
                .unwrap();
        }
    }
    agent_log_source_keys::prepare_for_test(&db).await;
    assert_eq!(
        store.agent_log_range(&agent.id, 0, 8, 8).await.unwrap(),
        entries
    );
    db.drop().await.unwrap();
}
