//! Real publisher/current-proof contract, including later progress and restart.
use super::capture_contract::{input, proposed};
use super::*;
use crate::store::agent_log_segments::{
    capture, current_publication, publication, schema, writer_commit,
};

async fn publish(f: &mut Fixture, key: &str) -> Publication {
    let (next, logs, mut operation) = proposed(f);
    operation.key = key.into();
    let mut session = transaction(f).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let captured = succeeded(capture::capture_transaction(&f.store, &f.agent, &mut scope).await);
    let mut request = input(f, &next, &logs, &operation);
    request.result_digest =
        publication::result_digest(&("domain-specific-result", next.revision)).unwrap();
    let prepared = succeeded(
        writer_commit::prepare(&f.store, &mut scope, &captured, request, None, None).await,
    );
    let receipt = succeeded(
        writer_commit::persist(
            &f.store,
            &mut scope,
            prepared,
            AggregateUpdate {
                filter: doc! {"_id":f.agent.id.as_str()},
                fields: agent_replacement_fields(&next).unwrap(),
            },
        )
        .await,
    );
    assert!(matches!(
        commit_transaction(scope.release()).await,
        CommitOutcome::Committed
    ));
    f.agent = next;
    receipt
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn current_publication_proves_later_progress_after_reopen_without_rewriting_original() {
    let mut f = fixture(2).await;
    let original = publish(&mut f, "original-publication").await;
    let original_row = collection(&f.store, PUBLICATIONS)
        .find_one(doc! {"_id":&original.id})
        .await
        .unwrap()
        .unwrap();
    let later = publish(&mut f, "later-publication").await;
    assert!(later.result_revision > original.result_revision);
    f.store = MongoSandboxStore::open(&f.client, &f.store.agents.namespace().db)
        .await
        .unwrap();
    let mut session = transaction(&f).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let proof = succeeded(current_publication::capture(&f.store, &f.agent, &mut scope).await);
    assert_eq!(
        proof.require_current(&f.store, &f.agent, &scope).unwrap(),
        &later
    );
    scope.release().abort_transaction().await.unwrap();
    assert_eq!(
        collection(&f.store, PUBLICATIONS)
            .find_one(doc! {"_id":&original.id})
            .await
            .unwrap()
            .unwrap(),
        original_row
    );
    assert_eq!(
        collection(&f.store, PUBLICATIONS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    assert_eq!(f.store.agent_log.count_documents(doc! {}).await.unwrap(), 4);
    f.destroy().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn current_publication_rejects_later_scope_same_session_other_database_and_changed_agent() {
    let mut f = fixture(2).await;
    publish(&mut f, "scope-publication").await;
    let mut session = transaction(&f).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let proof = succeeded(current_publication::capture(&f.store, &f.agent, &mut scope).await);
    let mut changed = f.agent.clone();
    changed.revision += 1;
    assert!(proof.require_current(&f.store, &changed, &scope).is_err());
    let foreign_name = format!("cma_current_foreign_{}", uuid::Uuid::new_v4().simple());
    let foreign = MongoSandboxStore::open(&f.client, &foreign_name)
        .await
        .unwrap();
    assert!(proof.require_current(&foreign, &f.agent, &scope).is_err());
    f.client.database(&foreign_name).drop().await.unwrap();
    scope.release().abort_transaction().await.unwrap();
    let scope = TransactionScope::start(&mut session).await.unwrap();
    assert!(proof.require_current(&f.store, &f.agent, &scope).is_err());
    scope.release().abort_transaction().await.unwrap();
    let mut other = transaction(&f).await;
    let scope = TransactionScope::start(&mut other).await.unwrap();
    assert!(proof.require_current(&f.store, &f.agent, &scope).is_err());
    scope.release().abort_transaction().await.unwrap();
    f.destroy().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn current_publication_refuses_missing_historical_or_mutated_current_evidence() {
    let mut f = fixture(2).await;
    let mut session = transaction(&f).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    assert!(current_publication::capture(&f.store, &f.agent, &mut scope)
        .await
        .is_err());
    scope.release().abort_transaction().await.unwrap();
    let receipt = publish(&mut f, "current-publication").await;
    let mut historical = receipt.clone();
    historical.format_version = 1;
    historical.agent_digest = None;
    let publications = collection(&f.store, PUBLICATIONS);
    publications
        .replace_one(
            doc! {"_id":&receipt.id},
            bson::serialize_to_document(&historical).unwrap(),
        )
        .await
        .unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    assert!(current_publication::capture(&f.store, &f.agent, &mut scope)
        .await
        .is_err());
    scope.release().abort_transaction().await.unwrap();
    // The historical receipt is readable but cannot authorize current attachment.
    assert_eq!(
        Publication::read(
            publications
                .find_one(doc! {"_id":&receipt.id})
                .await
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        historical
    );
    publications
        .replace_one(
            doc! {"_id":&receipt.id},
            bson::serialize_to_document(&receipt).unwrap(),
        )
        .await
        .unwrap();
    let mut changed = f.agent.clone();
    changed.owner = layer2_core::SubjectId::new("other-fixture-owner");
    f.store
        .agents
        .update_one(
            doc! {"_id":f.agent.id.as_str()},
            doc! {"$set":agent_replacement_fields(&changed).unwrap()},
        )
        .await
        .unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    assert!(current_publication::capture(&f.store, &changed, &mut scope)
        .await
        .is_err());
    scope.release().abort_transaction().await.unwrap();
    f.destroy().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn current_publication_abort_leaves_no_authoritative_witness() {
    let f = fixture(2).await;
    let mut session = transaction(&f).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let plan = f.prepared(&scope);
    let mutation = update(&plan);
    succeeded(persist(&f.store, &mut scope, plan, mutation).await);
    scope.release().abort_transaction().await.unwrap();
    unchanged(&f).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    assert!(current_publication::capture(&f.store, &f.agent, &mut scope)
        .await
        .is_err());
    scope.release().abort_transaction().await.unwrap();
    assert_eq!(
        collection(&f.store, PUBLICATIONS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    f.destroy().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn current_publication_uses_unique_head_lookup_and_refuses_index_drift_or_ambiguity() {
    let mut f = fixture(2).await;
    let receipt = publish(&mut f, "indexed-publication").await;
    let publications = collection(&f.store, PUBLICATIONS);
    let mut duplicate = receipt.clone();
    duplicate.publication_key = "different-key-same-head".into();
    duplicate.id = publication::identity(
        &duplicate.agent_id,
        &duplicate.incarnation,
        &duplicate.publication_key,
    );
    assert!(publications
        .insert_one(bson::serialize_to_document(&duplicate).unwrap())
        .await
        .is_err());
    let db = f.client.database(&f.store.agents.namespace().db);
    let filter = doc! {"agent_id":&receipt.agent_id,"incarnation":&receipt.incarnation,"head_revision":receipt.head_revision};
    let explain = db
        .run_command(doc! {"explain":{"find":PUBLICATIONS,"filter":filter,
        "hint":current_publication::INDEX,"limit":2_i64},"verbosity":"executionStats"})
        .await
        .unwrap();
    let stats = explain.get_document("executionStats").unwrap();
    assert_eq!(stats.get_i32("nReturned").unwrap(), 1);
    assert_eq!(stats.get_i32("totalDocsExamined").unwrap(), 1);
    assert_eq!(stats.get_i32("totalKeysExamined").unwrap(), 1);
    publications
        .drop_index(current_publication::INDEX)
        .await
        .unwrap();
    let mut session = transaction(&f).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    assert!(current_publication::capture(&f.store, &f.agent, &mut scope)
        .await
        .is_err());
    let _ = scope.release().abort_transaction().await;
    publications
        .create_index(
            mongodb::IndexModel::builder()
                .keys(doc! {"agent_id":1,"incarnation":1,"head_revision":1})
                .options(
                    mongodb::options::IndexOptions::builder()
                        .name(current_publication::INDEX.to_owned())
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
    assert!(schema::verify(&db).await.is_err());
    publications
        .insert_one(bson::serialize_to_document(&duplicate).unwrap())
        .await
        .unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    assert!(current_publication::capture(&f.store, &f.agent, &mut scope)
        .await
        .is_err());
    scope.release().abort_transaction().await.unwrap();
    f.destroy().await;
}
