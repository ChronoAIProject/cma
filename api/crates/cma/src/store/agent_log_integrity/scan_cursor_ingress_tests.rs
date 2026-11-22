use super::*;
use crate::store::agent_log_telemetry::{isolated_test, recorded_execution, snapshot};
use layer2_core::sandbox::{ingress_ownership::IngressWorkScope, AgentLogKind};
use mongodb::{Client, Collection, Database};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

#[path = "scan_cursor_ingress_races.rs"]
mod races;
use crate::store::agent_log_integrity::proof::summary_tests::policy;
use races::{client, database, entries, insert, options, Observation};

fn plan() -> AllocationPlan {
    AllocationPlan::checked("scan cursor fixture", &[50 * 1024 * 1024]).unwrap()
}

fn scan<'a, F, C: Clock + 'a>(
    collection: &'a Collection<Document>,
    id: &'a AgentId,
    last: u64,
    session: &'a mut mongodb::ClientSession,
    options: ScanOptions<C>,
    policy: &'a IngressOwnershipPolicy,
    visit: F,
) -> Result<ScanRead<'a>, AdmittedError>
where
    F: FnMut(SharedOwned<AgentLogEntry>) -> Result<(), OwnedWork<SandboxStoreError>> + Send + 'a,
{
    scan_collection_admitted(
        ScanCaller::Other,
        ScanInput {
            collection,
            agent_id: id,
            last_seq: last,
        },
        session,
        options,
        policy,
        plan(),
        visit,
    )
}

async fn marker(collection: &Collection<Document>, session: &mut mongodb::ClientSession) {
    assert!(collection
        .find_one(doc! {"_id":"pending"})
        .session(session)
        .await
        .unwrap()
        .is_some());
    assert!(collection
        .find_one(doc! {"_id":"pending"})
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn complete_cursor_modes_match_legacy_and_retain_summary_and_visitor_data() {
    let client = client().await;
    let db = database(&client);
    let collection = db.collection::<Document>("agent_log");
    let id = AgentId("agt_owned_scan".into());
    let mut expected = entries(&id, 600, 8192);
    expected[0].kind = AgentLogKind::AgentMessage {
        content: "small".into(),
    };
    let bytes = insert(&collection, &expected).await;
    assert!(bytes > 4 * 1024 * 1024);
    let (root, policy) = policy();
    let idle = root.available_bytes();
    for (cooperate, batches) in [(false, false), (true, false), (true, true)] {
        let mut session = client.start_session().await.unwrap();
        let original = scan_collection(
            ScanCaller::Other,
            &collection,
            &id,
            600,
            &mut session,
            options(cooperate).with_raw_batches(batches),
            |_| Ok(()),
        )
        .await
        .unwrap();
        let mut retained = Vec::new();
        let mut visits = 0;
        let (summary, execution) = isolated_test(recorded_execution(async {
            let summary = scan(
                &collection,
                &id,
                600,
                &mut session,
                options(cooperate).with_raw_batches(batches),
                &policy,
                |value| {
                    assert_eq!(*value, expected[visits]);
                    visits += 1;
                    if visits == 1 || visits == 600 {
                        retained.push(value);
                    }
                    Ok(())
                },
            )
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap();
            assert_eq!(&snapshot(ScanCaller::Other, true)[..3], &[1, 600, bytes]);
            assert_eq!(&snapshot(ScanCaller::Other, false)[..3], &[0, 0, 0]);
            summary
        }))
        .await;
        assert_eq!(*summary, original);
        assert_eq!(visits, 600);
        assert_eq!(execution.len(), 1);
        assert_eq!(execution[0].1 > 0, cooperate);
        assert!(summary.belongs_to(&policy));
        assert!(retained[1].requested_bytes() > retained[0].requested_bytes());
        let summary_bytes = idle
            - root.available_bytes()
            - retained.iter().map(|v| v.requested_bytes()).sum::<usize>();
        assert!(summary_bytes > 0 && summary_bytes < plan().requested_bytes());
        assert_eq!(
            policy
                .budget(IngressClass::Transformation)
                .available_bytes(),
            policy.bounds().transformation_bytes()
        );
        drop(summary);
        assert_eq!(
            idle - root.available_bytes(),
            retained.iter().map(|v| v.requested_bytes()).sum::<usize>()
        );
        drop(retained);
        assert_eq!(root.available_bytes(), idle);
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn refusal_before_query_and_original_row_failures_leave_caller_transactions_open() {
    let observation = Observation::default();
    let client = observation.client().await;
    let db = database(&client);
    let collection = db.collection::<Document>("agent_log");
    let writes = db.collection::<Document>("writes");
    writes.insert_one(doc! {"_id":"committed"}).await.unwrap();
    let id = AgentId("agt_owned_scan_failure".into());
    let expected = entries(&id, 200, 32);
    insert(&collection, &expected).await;
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = client.start_session().await.unwrap();
    session.start_transaction().await.unwrap();
    writes
        .insert_one(doc! {"_id":"pending"})
        .session(&mut session)
        .await
        .unwrap();
    observation.begin();
    drop(
        scan(
            &collection,
            &id,
            200,
            &mut session,
            options(true),
            &policy,
            |_| panic!("unpolled visitor"),
        )
        .unwrap(),
    );
    assert_eq!(root.available_bytes(), idle);
    for class in [IngressClass::Transformation, IngressClass::Data] {
        let pressure = policy
            .admit(
                class,
                AllocationPlan::checked("scan pressure", &[policy.budget(class).available_bytes()])
                    .unwrap(),
            )
            .unwrap();
        match scan(
            &collection,
            &id,
            200,
            &mut session,
            options(true),
            &policy,
            |_| panic!("refused visitor"),
        ) {
            Ok(work) => assert!(matches!(work.await, Err(AdmittedError::Refused(_)))),
            Err(error) => assert!(matches!(error, AdmittedError::Refused(_))),
        }
        drop(pressure);
    }
    assert_eq!(observation.commands("find"), 0);
    assert_eq!(observation.commands("abortTransaction"), 0);
    marker(&writes, &mut session).await;
    session.abort_transaction().await.unwrap();
    for case in 0..4 {
        collection.delete_many(doc! {}).await.unwrap();
        insert(&collection, &expected).await;
        match case {
            0 => {
                collection.delete_one(doc! {"seq":150_i64}).await.unwrap();
            }
            1 => {
                collection
                    .update_one(doc! {"seq":150_i64}, doc! {"$set":{"record":"bad"}})
                    .await
                    .unwrap();
            }
            2 => {
                let mut extra = expected[199].clone();
                extra.seq = 201;
                insert(&collection, &[extra]).await;
            }
            _ => {
                let mut deep = doc! {"leaf":true};
                for _ in 0..66 {
                    deep = doc! {"nested":deep};
                }
                collection
                    .update_one(doc! {"seq":150_i64}, doc! {"$set":{"unused":deep}})
                    .await
                    .unwrap();
            }
        }
        for batches in [false, true] {
            session.start_transaction().await.unwrap();
            writes
                .insert_one(doc! {"_id":"pending"})
                .session(&mut session)
                .await
                .unwrap();
            let original = scan_collection(
                ScanCaller::Other,
                &collection,
                &id,
                200,
                &mut session,
                options(true).with_raw_batches(batches),
                |_| Ok(()),
            )
            .await;
            let result = isolated_test(async {
                let result = scan(
                    &collection,
                    &id,
                    200,
                    &mut session,
                    options(true).with_raw_batches(batches),
                    &policy,
                    |_| Ok(()),
                )
                .unwrap()
                .await;
                assert_eq!(snapshot(ScanCaller::Other, true)[0], 0);
                assert_eq!(snapshot(ScanCaller::Other, false)[0], 1);
                result
            })
            .await;
            if case == 3 {
                assert!(matches!(result, Err(AdmittedError::Unsupported(_))));
                assert!(original.is_ok());
            } else {
                let TransactionLogError::Domain(expected) = original.unwrap_err() else {
                    panic!("domain fixture")
                };
                let error = result.unwrap().transpose().unwrap_err();
                assert!(matches!(&*error, ReadFailure::Domain(actual) if *actual == expected));
                assert!(error.belongs_to(&policy));
            }
            assert_eq!(root.available_bytes(), idle);
            marker(&writes, &mut session).await;
            session.abort_transaction().await.unwrap();
        }
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn driver_and_visitor_failures_retain_original_errors_without_successful_summary() {
    let control = client().await;
    let db = database(&control);
    let collection = db.collection::<Document>("agent_log");
    let id = AgentId("agt_owned_scan_driver".into());
    insert(&collection, &entries(&id, 200, 32)).await;
    let mut config =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    config.app_name = Some("cma-owned-scan-errors".into());
    let reader = Client::with_options(config).unwrap();
    let rows = reader
        .database(db.name())
        .collection::<Document>("agent_log");
    let admin = control.database("admin");
    let (root, policy) = policy();
    let idle = root.available_bytes();
    for batches in [false, true] {
        for command in ["find", "getMore"] {
            let mut session = reader.start_session().await.unwrap();
            session.start_transaction().await.unwrap();
            admin.run_command(doc! {"configureFailPoint":"failCommand", "mode":{"times":1}, "data":{
                "failCommands":[command], "appName":"cma-owned-scan-errors", "errorCode":112, "errorLabels":["TransientTransactionError"]
            }}).await.unwrap();
            let mut visits = 0;
            let result = scan(
                &rows,
                &id,
                200,
                &mut session,
                options(true).with_raw_batches(batches),
                &policy,
                |_| {
                    visits += 1;
                    Ok(())
                },
            )
            .unwrap()
            .await;
            admin
                .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
                .await
                .unwrap();
            let error = result.unwrap().transpose().unwrap_err();
            let ReadFailure::Driver(driver) = &*error else {
                panic!("original driver error")
            };
            assert!(driver.contains_label("TransientTransactionError"));
            let pointer = std::ptr::from_ref(driver.kind.as_ref());
            let moved = error.map(|error| match error {
                ReadFailure::Driver(error) => error,
                _ => unreachable!(),
            });
            assert_eq!(std::ptr::from_ref(moved.kind.as_ref()), pointer);
            assert_eq!(visits, if command == "find" { 0 } else { 101 });
            assert!(moved.belongs_to(&policy));
            drop(moved);
            assert_eq!(root.available_bytes(), idle);
            let _ = session.abort_transaction().await;
        }
        let mut session = reader.start_session().await.unwrap();
        let scope = IngressWorkScope::new(
            policy
                .admit(
                    IngressClass::Transformation,
                    AllocationPlan::checked("visitor error", &[4096]).unwrap(),
                )
                .unwrap(),
        );
        let mut original =
            Some(scope.initialize_owned(|| SandboxStoreError::Corrupt("visitor error".repeat(8))));
        let SandboxStoreError::Corrupt(message) = &**original.as_ref().unwrap() else {
            panic!("fixture")
        };
        let pointer = message.as_ptr();
        drop(scope);
        let mut visits = 0;
        let mut retained = None;
        let error = isolated_test(async {
            let error = scan(
                &rows,
                &id,
                200,
                &mut session,
                options(true).with_raw_batches(batches),
                &policy,
                |value| {
                    visits += 1;
                    if visits == 8 {
                        retained = Some(value);
                        Err(original.take().unwrap())
                    } else {
                        Ok(())
                    }
                },
            )
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap_err();
            assert_eq!(snapshot(ScanCaller::Other, true)[0], 0);
            assert_eq!(snapshot(ScanCaller::Other, false)[1], 8);
            error
        })
        .await;
        assert!(
            matches!(&*error, ReadFailure::Domain(SandboxStoreError::Corrupt(message)) if message.as_ptr()==pointer)
        );
        assert_eq!(visits, 8);
        drop(error);
        assert_eq!(
            idle - root.available_bytes(),
            retained.as_ref().unwrap().requested_bytes()
        );
        drop(retained);
        assert_eq!(root.available_bytes(), idle);
    }
    reader.shutdown().await;
    db.drop().await.unwrap();
}
