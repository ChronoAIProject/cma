use super::{doc, fixture::Fixture, policy, AllocationPlan, IngressClass};
use crate::store::agent_log_segments::transaction_scope::TransactionScope;
use crate::store::ingress_reads::cursor::DocumentSchema;
use layer2_core::sandbox::ingress_ownership::{AdmissionRefusal, AdmittedError};
use mongodb::bson::Document;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn a_new_transaction_sizes_the_replaced_row_without_reusing_earlier_admission() {
    let fixture = Fixture::new("read_replacement").await;
    let rows = fixture
        .store
        .client
        .database(&fixture.database)
        .collection::<Document>("ingress_rows");
    rows.insert_one(doc! {"_id":"row", "value":"small"})
        .await
        .unwrap();
    let (root, policy) = policy();
    let plan = || AllocationPlan::checked("replacement read", &[50 * 1024 * 1024]).unwrap();
    let mut session = fixture.store.client.start_session().await.unwrap();
    let mut transaction = TransactionScope::start(&mut session).await.unwrap();
    let first = transaction
        .find_one_admitted::<DocumentSchema, _>(&rows, &policy, plan(), || doc! {"_id":"row"})
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    assert_eq!(first.as_ref().unwrap().get_str("value").unwrap(), "small");
    drop(first);
    let other = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    other
        .database(&fixture.database)
        .collection::<Document>("ingress_rows")
        .update_one(
            doc! {"_id":"row"},
            doc! {"$set":{"value":"x".repeat(256 * 1024)}},
        )
        .await
        .unwrap();
    let prior = transaction
        .find_one_admitted::<DocumentSchema, _>(&rows, &policy, plan(), || doc! {"_id":"row"})
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    assert_eq!(prior.as_ref().unwrap().get_str("value").unwrap(), "small");
    drop(prior);
    transaction.release().abort_transaction().await.unwrap();
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "limited returned row storage",
                &[policy.bounds().data_bytes() - 8192],
            )
            .unwrap(),
        )
        .unwrap();
    let held = root.available_bytes();
    let mut later = TransactionScope::start(&mut session).await.unwrap();
    let result = later
        .find_one_admitted::<DocumentSchema, _>(&rows, &policy, plan(), || doc! {"_id":"row"})
        .unwrap()
        .await;
    assert!(matches!(
        result,
        Err(AdmittedError::Refused(AdmissionRefusal::AtCapacity { .. }))
    ));
    assert_eq!(root.available_bytes(), held);
    drop(pressure);
    let current = later
        .find_one_admitted::<DocumentSchema, _>(&rows, &policy, plan(), || doc! {"_id":"row"})
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    assert_eq!(
        current.as_ref().unwrap().get_str("value").unwrap().len(),
        256 * 1024
    );
    drop(current);
    later.release().abort_transaction().await.unwrap();
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    drop((session, rows));
    other.shutdown().await;
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn admitted_and_legacy_reads_use_the_same_ordinary_work_schema_predicate() {
    let fixture = Fixture::new("read_schema").await;
    let (root, policy) = policy();
    let plan = || AllocationPlan::checked("schema predicate read", &[50 * 1024 * 1024]).unwrap();
    let mut session = fixture.store.client.start_session().await.unwrap();
    let mut transaction = TransactionScope::start(&mut session).await.unwrap();
    let filter = || doc! {"$and":[{"_id":fixture.agent.sandbox_id.as_str()}, {"document_schema_version":1_i64}]};
    let legacy = transaction
        .find_one(&fixture.store.sandboxes, filter())
        .await
        .unwrap()
        .unwrap();
    let row = transaction
        .find_one_admitted::<DocumentSchema, _>(&fixture.store.sandboxes, &policy, plan(), filter)
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    assert_eq!(&**row.as_ref().unwrap(), &legacy);
    drop(row);
    let page = transaction
        .find_page_admitted(&fixture.store.sandboxes, &policy, plan(), 2, || {
            (filter(), doc! {"_id":1})
        })
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(*page[0], legacy);
    drop(page);
    transaction.release().abort_transaction().await.unwrap();
    drop(session);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    fixture.finish().await;
}
