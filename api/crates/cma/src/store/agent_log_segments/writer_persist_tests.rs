use super::*;
use crate::store::sandbox_mongo::{commit_transaction, CommitOutcome};
use mongodb::ClientSession;

#[path = "writer_test_fixture.rs"]
mod fixture;
use fixture::*;

#[path = "writer_bootstrap_mongo_tests.rs"]
mod bootstrap;

#[path = "writer_atomicity_mongo_tests.rs"]
mod atomicity;
#[path = "writer_capture_tests.rs"]
mod capture_contract;
#[path = "writer_current_proof_mongo_tests.rs"]
mod current_proof;
#[path = "writer_keyed_mongo_tests.rs"]
mod keyed_caller;
#[path = "writer_keyed_restart_mongo_tests.rs"]
mod keyed_restart;
#[path = "writer_keyed_settlement_mongo_tests.rs"]
mod keyed_settlement;
#[path = "writer_lifecycle_mongo_tests.rs"]
mod lifecycle_caller;
#[path = "writer_lifecycle_child_mongo_tests.rs"]
mod lifecycle_child;
#[path = "writer_original_callers_mongo_tests.rs"]
mod original_callers;
#[path = "writer_output_mongo_tests.rs"]
mod output_index;
#[path = "writer_unknown_mongo_tests.rs"]
mod uncertain_commit;

async fn transaction(f: &Fixture) -> ClientSession {
    f.client.start_session().await.unwrap()
}

#[track_caller]
fn succeeded<T>(result: Result<T, TransactionLogError>) -> T {
    match result {
        Ok(value) => value,
        Err(TransactionLogError::Domain(error)) => panic!("publication domain error: {error}"),
        Err(TransactionLogError::Driver(error)) => panic!("publication driver error: {error}"),
    }
}

fn update(plan: &PreparedPublication) -> AggregateUpdate {
    AggregateUpdate {
        filter: doc! {"_id":plan.current.id.as_str()},
        fields: agent_replacement_fields(&plan.next).unwrap(),
    }
}

async fn unchanged(f: &Fixture) {
    let row = f
        .store
        .agents
        .find_one(doc! {"_id":f.agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(crate::store::agents::read_agent(&row).unwrap(), f.agent);
    assert_eq!(
        f.store.agent_log.count_documents(doc! {}).await.unwrap(),
        f.head.last_seq as u64
    );
    assert_eq!(
        collection(&f.store, SEGMENTS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let head = collection(&f.store, HEADS)
        .find_one(doc! {"_id":f.agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(Head::read(head).unwrap(), f.head);
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn writer_rollover_commits_original_batch_head_projection_and_receipt_together() {
    let f = fixture(10_000).await;
    let mut session = transaction(&f).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let plan = f.prepared(&scope);
    let expected = plan.next.clone();
    let mutation = update(&plan);
    let receipt = succeeded(persist(&f.store, &mut scope, plan, mutation).await);
    assert!(matches!(
        commit_transaction(scope.release()).await,
        CommitOutcome::Committed
    ));
    let row = f
        .store
        .agents
        .find_one(doc! {"_id":f.agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(crate::store::agents::read_agent(&row).unwrap(), expected);
    assert_eq!(
        f.store.agent_log.count_documents(doc! {}).await.unwrap(),
        10_001
    );
    let head = Head::read(
        collection(&f.store, HEADS)
            .find_one(doc! {"_id":f.agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        (head.active_ordinal, head.active_count, head.last_seq),
        (1, 1, 10_001)
    );
    assert_eq!(
        collection(&f.store, SEGMENTS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    let proof = collection(&f.store, PROJECTIONS)
        .find_one(doc! {"_id":f.agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(proof.get_i64("through_seq").unwrap(), head.last_seq);
    assert_eq!(proof.get_i64("head_revision").unwrap(), head.revision);
    let committed = Publication::read(
        collection(&f.store, PUBLICATIONS)
            .find_one(doc! {"_id":&receipt.id})
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(committed.result_digest, receipt.result_digest);
    f.destroy().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn writer_abort_after_rollover_and_late_receipt_conflict_leave_no_partial_effects() {
    let f = fixture(10_000).await;
    let mut session = transaction(&f).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let plan = f.prepared(&scope);
    let mutation = update(&plan);
    let receipt = succeeded(persist(&f.store, &mut scope, plan, mutation).await);
    scope.release().abort_transaction().await.unwrap();
    unchanged(&f).await;
    assert_eq!(
        collection(&f.store, PUBLICATIONS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    // Force a late immutable-receipt uniqueness failure after other writes.
    collection(&f.store, PUBLICATIONS)
        .insert_one(bson::serialize_to_document(&receipt).unwrap())
        .await
        .unwrap();
    let mut session = transaction(&f).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let plan = f.prepared(&scope);
    let mutation = update(&plan);
    assert!(persist(&f.store, &mut scope, plan, mutation).await.is_err());
    let _ = scope.release().abort_transaction().await;
    unchanged(&f).await;
    f.destroy().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn writer_rejects_stale_domain_fence_mismatched_result_and_different_session() {
    let f = fixture(2).await;
    let mut session = transaction(&f).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let plan = f.prepared(&scope);
    let mut mutation = update(&plan);
    mutation.filter.insert("work_lease_fence", 999_i64);
    assert!(persist(&f.store, &mut scope, plan, mutation).await.is_err());
    scope.release().abort_transaction().await.unwrap();
    let mut session = transaction(&f).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let plan = f.prepared(&scope);
    let mut mutation = update(&plan);
    mutation.fields.insert("last_seq", 999_i64);
    assert!(persist(&f.store, &mut scope, plan, mutation).await.is_err());
    scope.release().abort_transaction().await.unwrap();
    let mut original = transaction(&f).await;
    let original_scope = TransactionScope::start(&mut original).await.unwrap();
    let plan = f.prepared(&original_scope);
    let mutation = update(&plan);
    let mut another = transaction(&f).await;
    let mut another_scope = TransactionScope::start(&mut another).await.unwrap();
    assert!(persist(&f.store, &mut another_scope, plan, mutation)
        .await
        .is_err());
    original_scope.release().abort_transaction().await.unwrap();
    another_scope.release().abort_transaction().await.unwrap();
    unchanged(&f).await;
    f.destroy().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn writer_rejects_previous_transaction_plan_on_same_session_and_other_database() {
    let f = fixture(2).await;
    let mut session = transaction(&f).await;
    let scope = TransactionScope::start(&mut session).await.unwrap();
    let plan = f.prepared(&scope);
    let mutation = update(&plan);
    scope.release().abort_transaction().await.unwrap();
    let mut later = TransactionScope::start(&mut session).await.unwrap();
    assert!(persist(&f.store, &mut later, plan, mutation).await.is_err());
    later.release().abort_transaction().await.unwrap();
    unchanged(&f).await;

    let other = fixture(2).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let plan = f.prepared(&scope);
    let mutation = update(&plan);
    assert!(persist(&other.store, &mut scope, plan, mutation)
        .await
        .is_err());
    scope.release().abort_transaction().await.unwrap();
    unchanged(&f).await;
    unchanged(&other).await;
    other.destroy().await;
    f.destroy().await;
}

#[path = "writer_boundary_retry_mongo_tests.rs"]
mod boundary_retry;
#[path = "writer_precommit_custody_mongo_tests.rs"]
mod precommit_custody;
