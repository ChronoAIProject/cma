use super::*;
use crate::store::account_deletions::worker_schema as schema;
use crate::store::migrations::account_erasure_work as migration;

#[test]
fn erasure_schema_is_explicit_with_separate_additive_authority() {
    let args = vec!["--migration-plan".into(), migration::ID.into()];
    assert_eq!(invocation(&args).unwrap().0, Step::AccountErasureWork);
    assert_eq!(Step::AccountErasureWork.collections(), schema::COLLECTIONS);
    assert_ne!(
        Step::AccountErasureWork.checksum(),
        Step::Activity.checksum()
    );
    let status = |grants| doc! {"authInfo":{"authenticatedUsers":[{"user":"fixture","db":"fixture"}],"authenticatedUserPrivileges":grants}};
    let grants = authority::role_for("fixture", true, Step::AccountErasureWork)
        .get_array("privileges")
        .unwrap()
        .clone();
    authority::verify_for(
        &status(grants.clone()),
        "fixture",
        MongoAuthorityMode::Isolated,
        true,
        Step::AccountErasureWork,
    )
    .unwrap();
    for action in [
        "remove",
        "dropCollection",
        "dropIndex",
        "collMod",
        "insert",
        "update",
    ] {
        let mut changed = grants.clone();
        changed[1]
            .as_document_mut()
            .unwrap()
            .get_array_mut("actions")
            .unwrap()
            .push(action.into());
        assert!(authority::verify_for(
            &status(changed),
            "fixture",
            MongoAuthorityMode::Isolated,
            true,
            Step::AccountErasureWork
        )
        .is_err());
    }
    let mut runtime = crate::store::isolation::role("fixture")
        .get_array("privileges")
        .unwrap()
        .clone();
    for grant in &runtime {
        let grant = grant.as_document().unwrap();
        if schema::COLLECTIONS.contains(
            &grant
                .get_document("resource")
                .unwrap()
                .get_str("collection")
                .unwrap(),
        ) {
            assert_eq!(
                grant.get_array("actions").unwrap(),
                &vec![
                    "find".into(),
                    "insert".into(),
                    "update".into(),
                    "listIndexes".into()
                ]
            );
        }
    }
    crate::store::isolation::validate(&status(runtime.clone()), "fixture").unwrap();
    runtime.retain(|grant| {
        let collection = grant
            .as_document()
            .unwrap()
            .get_document("resource")
            .unwrap()
            .get_str("collection")
            .unwrap();
        !schema::COLLECTIONS.contains(&collection) && collection != ledger::COLLECTION
    });
    crate::store::isolation::validate(&status(runtime), "fixture").unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn erasure_preparation_is_read_only_until_explicit_apply_and_idempotent_afterward() {
    let (client, database, _) = fixture().await;
    let ledger = ledger::Ledger::for_step(client, database.clone(), Step::AccountErasureWork);
    assert_eq!(migration::inspect(&database).await.unwrap().len(), 2);
    assert!(migration::prepared(&database).await.is_err());
    assert!(database.list_collection_names().await.unwrap().is_empty());
    let result = migration::apply(&ledger, &database, &identity())
        .await
        .unwrap();
    assert_eq!(result["applied_schema_steps"], 4);
    migration::prepared(&database).await.unwrap();
    assert!(migration::inspect(&database).await.unwrap().is_empty());
    assert_eq!(
        migration::apply(&ledger, &database, &identity())
            .await
            .unwrap()["outcome"],
        "already_completed"
    );
    for name in schema::COLLECTIONS {
        assert_eq!(
            database
                .collection::<Document>(name)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
    }
    assert_eq!(
        database
            .collection::<Document>(ledger::COLLECTION)
            .count_documents(doc! {"kind":"attempt"})
            .await
            .unwrap(),
        1
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn erasure_preparation_refuses_reordered_keys_and_completed_drift_without_repair() {
    let (client, database, _) = fixture().await;
    let ledger = ledger::Ledger::for_step(client, database.clone(), Step::AccountErasureWork);
    migration::create(&database, schema::EFFECTS, doc! {"kind":"collection"})
        .await
        .unwrap();
    let mut reordered = schema::indexes(schema::EFFECTS)[0].clone();
    reordered.insert("key", doc! {"journal_sequence":1,"deletion_id":1});
    assert!(migration::create(&database, schema::EFFECTS, reordered)
        .await
        .is_err());
    migration::apply(&ledger, &database, &identity())
        .await
        .unwrap();
    database
        .collection::<Document>(schema::EFFECTS)
        .drop_index("erasure_operation_sequence_v1")
        .await
        .unwrap();
    assert!(migration::apply(&ledger, &database, &identity())
        .await
        .is_err());
    assert_eq!(migration::inspect(&database).await.unwrap().len(), 1);
    assert_eq!(
        database
            .collection::<Document>(ledger::COLLECTION)
            .count_documents(doc! {"kind":"attempt"})
            .await
            .unwrap(),
        1
    );
    database.drop().await.unwrap();
}
