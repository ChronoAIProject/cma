use std::sync::atomic::{AtomicUsize, Ordering};

use layer2_core::sandbox::ingress_ownership::{AdmissionRefusal, AdmittedError};
use mongodb::bson::Document;

use super::{doc, fixture::Fixture, policy, AllocationPlan, IngressClass};
use crate::store::agent_log_segments::transaction_scope::TransactionScope;
use crate::store::ingress_reads::{cursor::DocumentSchema, error::ReadFailure};

fn read_plan() -> AllocationPlan {
    AllocationPlan::checked("synthetic transaction read", &[50 * 1024 * 1024]).unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn admitted_transaction_reads_preserve_order_own_writes_and_retained_rows() {
    let fixture = Fixture::new("transaction_reads").await;
    let rows = fixture
        .store
        .client
        .database(&fixture.database)
        .collection::<Document>("ingress_rows");
    rows.insert_many([
        doc! {"_id":"a", "value":"first"},
        doc! {"_id":"b", "value":"second"},
    ])
    .await
    .unwrap();
    let (root, policy) = policy();
    let mut session = fixture.store.client.start_session().await.unwrap();
    let mut transaction = TransactionScope::start(&mut session).await.unwrap();
    let legacy = transaction
        .find_page(&rows, doc! {}, doc! {"_id":-1}, 2)
        .await
        .unwrap();
    let page = transaction
        .find_page_admitted(&rows, &policy, read_plan(), 2, || {
            (doc! {}, doc! {"_id":-1})
        })
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    assert!(page.belongs_to(&policy));
    assert_eq!(
        page.iter().map(|row| &**row).collect::<Vec<_>>(),
        legacy.iter().collect::<Vec<_>>()
    );
    assert_eq!(
        policy
            .budget(IngressClass::Transformation)
            .available_bytes(),
        policy.bounds().transformation_bytes()
    );
    let held = root.available_bytes();
    let first = page[0].clone();
    drop(page);
    assert!(root.available_bytes() > held);
    assert!(root.available_bytes() < root.capacity_bytes() - root.metadata_bytes());
    assert_eq!(first.get_str("value").unwrap(), "second");
    transaction
        .update_one(&rows, doc! {"_id":"b"}, doc! {"$set":{"value":"changed"}})
        .await
        .unwrap();
    let result = transaction
        .find_one_admitted::<DocumentSchema, _>(&rows, &policy, read_plan(), || doc! {"_id":"b"})
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    assert_eq!(
        result.as_ref().unwrap().get_str("value").unwrap(),
        "changed"
    );
    assert_eq!(first.get_str("value").unwrap(), "second");
    drop((result, first));
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
    transaction.release().abort_transaction().await.unwrap();
    // A later transaction on this same session observes the original state.
    let mut later = TransactionScope::start(&mut session).await.unwrap();
    let row = later
        .find_one_admitted::<DocumentSchema, _>(&rows, &policy, read_plan(), || doc! {"_id":"b"})
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    assert_eq!(row.as_ref().unwrap().get_str("value").unwrap(), "second");
    drop(row);
    later.release().abort_transaction().await.unwrap();
    drop((session, rows));
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn transaction_read_refusal_precedes_query_construction_and_leaves_legacy_reads_usable() {
    let fixture = Fixture::new("transaction_refusal").await;
    let (root, policy) = policy();
    let mut session = fixture.store.client.start_session().await.unwrap();
    let mut transaction = TransactionScope::start(&mut session).await.unwrap();
    let calls = AtomicUsize::new(0);
    let occupied = policy
        .admit(
            IngressClass::Transformation,
            AllocationPlan::checked("occupied work", &[policy.bounds().transformation_bytes()])
                .unwrap(),
        )
        .unwrap();
    let held = root.available_bytes();
    assert!(matches!(
        transaction.find_one_admitted::<DocumentSchema, _>(
            &fixture.store.agents,
            &policy,
            read_plan(),
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                doc! {}
            }
        ),
        Err(AdmittedError::Refused(AdmissionRefusal::AtCapacity { .. }))
    ));
    assert_eq!(root.available_bytes(), held);
    drop(occupied);
    let unpolled = transaction
        .find_one_admitted::<DocumentSchema, _>(&fixture.store.agents, &policy, read_plan(), || {
            calls.fetch_add(1, Ordering::SeqCst);
            doc! {}
        })
        .unwrap();
    drop(unpolled);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
    let occupied = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked("occupied data", &[policy.bounds().data_bytes()]).unwrap(),
        )
        .unwrap();
    let held = root.available_bytes();
    let result = transaction
        .find_page_admitted(&fixture.store.agents, &policy, read_plan(), 2, || {
            calls.fetch_add(1, Ordering::SeqCst);
            (doc! {}, doc! {})
        })
        .unwrap()
        .await;
    assert!(matches!(
        result,
        Err(AdmittedError::Refused(AdmissionRefusal::AtCapacity { .. }))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(root.available_bytes(), held);
    drop(occupied);
    for limit in [0, 257, u32::MAX] {
        let error = transaction
            .find_page_admitted(&fixture.store.agents, &policy, read_plan(), limit, || {
                panic!("invalid page constructed a query")
            })
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap_err();
        assert!(matches!(
            &*error,
            ReadFailure::InvalidPage("transaction page limit must be 1..=256")
        ));
    }
    assert!(transaction
        .find_one(
            &fixture.store.agents,
            doc! {"_id":fixture.agent.id.as_str()}
        )
        .await
        .unwrap()
        .is_some());
    transaction.release().abort_transaction().await.unwrap();
    drop(session);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn transaction_read_keeps_original_driver_labels_and_error_allocation() {
    let mut options =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    let app_name = "cma-ingress-transaction-error";
    options.app_name = Some(app_name.into());
    let fixture = Fixture::with_client(
        mongodb::Client::with_options(options).unwrap(),
        "transaction_error",
    )
    .await;
    let admin = fixture.store.client.database("admin");
    let (root, policy) = policy();
    let mut session = fixture.store.client.start_session().await.unwrap();
    for paged in [false, true] {
        let mut transaction = TransactionScope::start(&mut session).await.unwrap();
        admin
            .run_command(
                doc! {"configureFailPoint":"failCommand", "mode":{"times":1}, "data":{
                    "failCommands":["find"], "appName":app_name, "errorCode":112,
                    "errorLabels":["TransientTransactionError"],
                }},
            )
            .await
            .unwrap();
        let error = if paged {
            transaction
                .find_page_admitted(&fixture.store.agents, &policy, read_plan(), 2, || {
                    (doc! {}, doc! {})
                })
                .unwrap()
                .await
                .unwrap()
                .transpose()
                .unwrap_err()
        } else {
            transaction
                .find_one_admitted::<DocumentSchema, _>(
                    &fixture.store.agents,
                    &policy,
                    read_plan(),
                    || doc! {},
                )
                .unwrap()
                .await
                .unwrap()
                .transpose()
                .unwrap_err()
        };
        admin
            .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
            .await
            .unwrap();
        let ReadFailure::Driver(driver) = &*error else {
            panic!("driver error became domain/admission failure")
        };
        assert!(driver.contains_label("TransientTransactionError"));
        assert!(
            matches!(driver.kind.as_ref(), mongodb::error::ErrorKind::Command(command) if command.code == 112)
        );
        let address = std::ptr::from_ref(driver.kind.as_ref());
        let moved = error.map(|error| match error {
            ReadFailure::Driver(driver) => driver,
            _ => unreachable!(),
        });
        assert_eq!(std::ptr::from_ref(moved.kind.as_ref()), address);
        assert!(moved.belongs_to(&policy));
        assert!(root.available_bytes() < root.capacity_bytes() - root.metadata_bytes());
        drop(moved);
        assert_eq!(
            root.available_bytes(),
            root.capacity_bytes() - root.metadata_bytes()
        );
        let _ = transaction.release().abort_transaction().await;
    }
    drop((session, admin));
    fixture.finish().await;
}
