use super::{fixtures::*, *};
use crate::store::agent_log_integrity::proof::summary_tests::policy;
use crate::store::agent_log_telemetry::{isolated_test, snapshot};
use layer2_core::sandbox::AgentLogKind;
use mongodb::Client;
use std::{
    future::{poll_fn, Future},
    task::Poll,
};

#[test]
fn container_never_grows_and_entries_keep_their_original_independent_owners() {
    let (root, policy) = policy();
    let idle = root.available_bytes();
    assert!(CapturedEntries::plan(usize::MAX).is_err());
    for count in [0, 1, 4, 5, 17, 129] {
        let values = entries(count, 32);
        let mut capture = CapturedEntries::new(count as usize, &policy).unwrap();
        let address = capture.entries.as_ptr();
        for value in &values {
            capture.push(owned(value, &policy)).unwrap();
        }
        assert_eq!(capture.entries.as_ptr(), address);
        assert_eq!(capture.entries.len(), count as usize);
        let retained = capture.as_slice().first().cloned();
        let extra = owned(&entries(1, 32)[0], &policy);
        let error = capture.push(extra).unwrap_err();
        assert_eq!(*error, SandboxStoreError::SequenceConflict);
        drop(error);
        assert_eq!(capture.entries.as_ptr(), address);
        drop(capture);
        assert_eq!(
            idle - root.available_bytes(),
            retained.as_ref().map_or(0, |v| v.requested_bytes())
        );
        drop(retained);
        assert_eq!(root.available_bytes(), idle);
    }
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "pressure",
                &[policy.budget(IngressClass::Data).available_bytes()],
            )
            .unwrap(),
        )
        .unwrap();
    assert!(matches!(
        CapturedEntries::new(1, &policy),
        Err(AdmittedError::Refused(_))
    ));
    drop(pressure);
    assert_eq!(root.available_bytes(), idle);
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn complete_capture_preserves_order_summary_and_independent_component_lifetimes() {
    let (client, db, collection) = fixture().await;
    let id = AgentId("agt_history_capture".into());
    let (root, policy) = policy();
    let idle = root.available_bytes();
    for count in [0, 1, 600] {
        let values = entries(count, 1024);
        collection.delete_many(doc! {}).await.unwrap();
        insert(&collection, &values).await;
        for batches in [false, true] {
            let mut session = client.start_session().await.unwrap();
            let original = scan_collection(
                ScanCaller::Other,
                &collection,
                &id,
                count,
                &mut session,
                options(false, batches),
                |_| Ok(()),
            )
            .await
            .unwrap();
            let result = capture(
                &collection,
                &id,
                count,
                &mut session,
                options(false, batches),
                &policy,
            )
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap();
            assert_eq!(*result.summary, original);
            assert_eq!(result.entries.as_slice().len(), values.len());
            for (actual, expected) in result.entries.as_slice().iter().zip(&values) {
                assert_eq!(**actual, *expected);
                assert!(actual.belongs_to(&policy));
            }
            assert!(result.summary.belongs_to(&policy));
            assert_eq!(
                policy
                    .budget(IngressClass::Transformation)
                    .available_bytes(),
                policy.bounds().transformation_bytes()
            );
            let kept = result.entries.as_slice().last().cloned();
            let summary = result.map(|history| history.summary);
            assert_eq!(summary.entry_count, count);
            drop(summary);
            assert_eq!(
                idle - root.available_bytes(),
                kept.as_ref().map_or(0, |v| v.requested_bytes())
            );
            drop(kept);
            assert_eq!(root.available_bytes(), idle);
        }
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn refusal_and_original_partial_scan_errors_release_capture_without_ending_transactions() {
    let (client, db, collection) = fixture().await;
    let writes = db.collection::<Document>("writes");
    writes.insert_one(doc! {"_id":"committed"}).await.unwrap();
    let values = entries(200, 32);
    let id = &values[0].agent_id;
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = client.start_session().await.unwrap();
    drop(
        capture(
            &collection,
            id,
            200,
            &mut session,
            options(false, true),
            &policy,
        )
        .unwrap(),
    );
    assert_eq!(root.available_bytes(), idle);
    for class in [IngressClass::Transformation, IngressClass::Data] {
        let pressure = policy
            .admit(
                class,
                AllocationPlan::checked("pressure", &[policy.budget(class).available_bytes()])
                    .unwrap(),
            )
            .unwrap();
        match capture(
            &collection,
            id,
            200,
            &mut session,
            options(false, true),
            &policy,
        ) {
            Err(error) => assert!(matches!(error, AdmittedError::Refused(_))),
            Ok(work) => assert!(matches!(work.await, Err(AdmittedError::Refused(_)))),
        }
        drop(pressure);
        assert_eq!(root.available_bytes(), idle);
    }
    let error = capture(
        &collection,
        id,
        u64::MAX,
        &mut session,
        options(false, true),
        &policy,
    )
    .unwrap()
    .await
    .unwrap()
    .transpose()
    .unwrap_err();
    assert!(matches!(
        *error,
        ReadFailure::Domain(SandboxStoreError::AgentLogCapacityExceeded)
    ));
    drop(error);
    assert_eq!(root.available_bytes(), idle);
    for case in 0..4 {
        collection.delete_many(doc! {}).await.unwrap();
        insert(&collection, &values).await;
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
                let mut extra = values[199].clone();
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
                id,
                200,
                &mut session,
                options(false, batches),
                |_| Ok(()),
            )
            .await;
            let result = isolated_test(async {
                let result = capture(
                    &collection,
                    id,
                    200,
                    &mut session,
                    options(false, batches),
                    &policy,
                )
                .unwrap()
                .await;
                assert_eq!(snapshot(ScanCaller::Other, true)[0], 0);
                result
            })
            .await;
            if case == 3 {
                assert!(matches!(result, Err(AdmittedError::Unsupported(_))));
                assert!(original.is_ok());
            } else {
                let TransactionLogError::Domain(expected) = original.unwrap_err() else {
                    panic!("fixture")
                };
                let error = result.unwrap().transpose().unwrap_err();
                assert!(matches!(&*error,ReadFailure::Domain(actual) if *actual==expected));
                assert!(error.belongs_to(&policy));
                drop(error);
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
async fn captured_snapshot_and_transaction_views_survive_replacements_across_get_more() {
    let (client, db, collection) = fixture().await;
    let values = entries(300, 128);
    let id = &values[0].agent_id;
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let pause_bytes = retained_bytes(&values, 8, &policy);
    for snapshot in [false, true] {
        for batches in [false, true] {
            collection.delete_many(doc! {}).await.unwrap();
            insert(&collection, &values).await;
            let mut session = client.start_session().snapshot(snapshot).await.unwrap();
            if !snapshot {
                session
                    .start_transaction()
                    .read_concern(ReadConcern::snapshot())
                    .await
                    .unwrap();
            }
            let mut work = Box::pin(
                capture(
                    &collection,
                    id,
                    300,
                    &mut session,
                    options(snapshot, batches),
                    &policy,
                )
                .unwrap(),
            );
            poll_fn(|cx| {
                assert!(work.as_mut().poll(cx).is_pending());
                if policy.bounds().data_bytes()
                    - policy.budget(IngressClass::Data).available_bytes()
                    >= pause_bytes
                {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
            assert_eq!(
                policy.bounds().data_bytes() - policy.budget(IngressClass::Data).available_bytes(),
                pause_bytes
            );
            let mut changed = values[149].clone();
            changed.kind = AgentLogKind::AgentMessage {
                content: "changed".into(),
            };
            collection
                .replace_one(doc! {"seq":150_i64}, log_document(&changed).unwrap())
                .await
                .unwrap();
            let mut extra = values[299].clone();
            extra.seq = 301;
            insert(&collection, &[extra]).await;
            let result = work.await.unwrap().transpose().unwrap();
            assert_eq!(result.summary.entry_count, 300);
            for (actual, expected) in result.entries.as_slice().iter().zip(&values) {
                assert_eq!(**actual, *expected);
            }
            drop(result);
            assert_eq!(root.available_bytes(), idle);
            if !snapshot {
                session.abort_transaction().await.unwrap();
            }
        }
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn cancelled_and_data_refused_captures_release_all_partial_entries_and_preserve_session() {
    let (client, db, collection) = fixture().await;
    let writes = db.collection::<Document>("writes");
    writes.insert_one(doc! {"_id":"committed"}).await.unwrap();
    let values = entries(400, 128);
    let id = &values[0].agent_id;
    insert(&collection, &values).await;
    let (root, policy) = policy();
    let idle = root.available_bytes();
    for batches in [false, true] {
        for (through, pressure) in [(8, false), (112, false), (8, true)] {
            let pause_bytes = retained_bytes(&values, through, &policy);
            let mut session = client.start_session().await.unwrap();
            session.start_transaction().await.unwrap();
            writes
                .insert_one(doc! {"_id":"pending"})
                .session(&mut session)
                .await
                .unwrap();
            isolated_test(async {
                let mut work = Box::pin(
                    capture(
                        &collection,
                        id,
                        400,
                        &mut session,
                        options(false, batches),
                        &policy,
                    )
                    .unwrap(),
                );
                poll_fn(|cx| {
                    assert!(work.as_mut().poll(cx).is_pending());
                    if policy.bounds().data_bytes()
                        - policy.budget(IngressClass::Data).available_bytes()
                        >= pause_bytes
                    {
                        Poll::Ready(())
                    } else {
                        Poll::Pending
                    }
                })
                .await;
                assert_eq!(
                    policy.bounds().data_bytes()
                        - policy.budget(IngressClass::Data).available_bytes(),
                    pause_bytes
                );
                if pressure {
                    let hold = policy
                        .admit(
                            IngressClass::Data,
                            AllocationPlan::checked(
                                "mid-capture pressure",
                                &[policy.budget(IngressClass::Data).available_bytes()],
                            )
                            .unwrap(),
                        )
                        .unwrap();
                    assert!(matches!(work.await, Err(AdmittedError::Refused(_))));
                    drop(hold);
                } else {
                    drop(work);
                }
                assert_eq!(snapshot(ScanCaller::Other, true)[0], 0);
                assert_eq!(
                    snapshot(ScanCaller::Other, false)[1],
                    (through + usize::from(pressure)) as u64
                );
            })
            .await;
            assert_eq!(root.available_bytes(), idle);
            marker(&writes, &mut session).await;
            session.abort_transaction().await.unwrap();
        }
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn capture_preserves_original_driver_errors_after_partial_delivery() {
    let (control, db, collection) = fixture().await;
    let values = entries(200, 32);
    let id = &values[0].agent_id;
    insert(&collection, &values).await;
    let mut config =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    config.app_name = Some("cma-capture-errors".into());
    let reader = Client::with_options(config).unwrap();
    let rows = reader.database(db.name()).collection("agent_log");
    let admin = control.database("admin");
    let (root, policy) = policy();
    let idle = root.available_bytes();
    for batches in [false, true] {
        for command in ["find", "getMore"] {
            let mut session = reader.start_session().await.unwrap();
            session.start_transaction().await.unwrap();
            admin.run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},"data":{
            "failCommands":[command],"appName":"cma-capture-errors","errorCode":112,"errorLabels":["TransientTransactionError"]
        }}).await.unwrap();
            let error = isolated_test(async {
                let error = capture(
                    &rows,
                    id,
                    200,
                    &mut session,
                    options(false, batches),
                    &policy,
                )
                .unwrap()
                .await
                .unwrap()
                .transpose()
                .unwrap_err();
                assert_eq!(
                    snapshot(ScanCaller::Other, false)[1],
                    if command == "find" { 0 } else { 101 }
                );
                error
            })
            .await;
            admin
                .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
                .await
                .unwrap();
            let ReadFailure::Driver(driver) = &*error else {
                panic!("driver error")
            };
            assert!(driver.contains_label("TransientTransactionError"));
            let pointer = std::ptr::from_ref(driver.kind.as_ref());
            let moved = error.map(|value| match value {
                ReadFailure::Driver(error) => error,
                _ => unreachable!(),
            });
            assert_eq!(std::ptr::from_ref(moved.kind.as_ref()), pointer);
            assert!(moved.belongs_to(&policy));
            drop(moved);
            assert_eq!(root.available_bytes(), idle);
            let _ = session.abort_transaction().await;
        }
    }
    reader.shutdown().await;
    db.drop().await.unwrap();
}

#[path = "scan_capture_assessment_ingress_tests.rs"]
mod assessment;

#[path = "scan_capture_transaction_ingress_tests.rs"]
mod transaction;
