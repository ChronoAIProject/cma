use super::super::fixtures::{owned, plan};
use super::super::*;
use crate::store::agent_log_integrity::proof::{
    ingress::assert_open_transaction, summary_tests::policy,
};
use crate::store::agent_log_telemetry::isolated_test;
use crate::store::agents::agent_document;
use crate::store::sandbox_maintenance::mongo_integration_tests::database;
use crate::store::sandbox_mongo::start_transaction;
use layer2_core::sandbox::AgentLogKind;
use std::{
    future::{poll_fn, Future},
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    task::Poll,
};

use super::super::report_fixtures::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn captured_report_preserves_agent_proof_and_oversized_head_precedence() {
    let (client, name, mut store) = database().await;
    let (agent, _) = seed(&store, 0).await;
    let (reference_root, reference_policy) = policy();
    let (root, policy) = policy();
    let idle = root.available_bytes();
    for percent in [0, 100] {
        store = store.with_fleet_log_percentage(percent);
        for case in [
            "valid",
            "absent",
            "missing-agent",
            "quarantine",
            "identity",
            "malformed",
            "segmented",
            "bad-agent",
            "unsupported",
        ] {
            let row = report_row(&agent, case);
            store
                .agents
                .replace_one(doc! {"_id":agent.id.as_str()}, row.clone())
                .upsert(true)
                .await
                .unwrap();
            if case == "missing-agent" {
                store
                    .agents
                    .delete_one(doc! {"_id":agent.id.as_str()})
                    .await
                    .unwrap();
            }
            for head in [0, 1, u64::MAX] {
                let mut session = client.start_session().await.unwrap();
                begin(&store, &mut session).await;
                let (expected, metrics) = isolated_test(async {
                    let r = assessment::scan_report_only_admitted(
                        ScanCaller::Other,
                        input(&store, &agent, head),
                        &mut session,
                        false,
                        &reference_policy,
                        plan(),
                        |_| Ok(()),
                    )
                    .unwrap()
                    .await;
                    (r, counts())
                })
                .await;
                isolated_test(async {
                    let actual = capture_report_only_admitted(
                        ScanCaller::Other,
                        input(&store, &agent, head),
                        &mut session,
                        false,
                        &policy,
                        plan(),
                    )
                    .unwrap()
                    .await;
                    compare(expected, actual);
                    assert_eq!(counts(), metrics, "{case}/{head}");
                })
                .await;
                assert_open_transaction(&store, &mut session, "capture-report-marker").await;
                session.abort_transaction().await.unwrap();
                assert_eq!(root.available_bytes(), idle);
                assert_eq!(reference_root.available_bytes(), idle);
            }
            assert_eq!(
                store
                    .agents
                    .find_one(doc! {"_id":agent.id.as_str()})
                    .await
                    .unwrap(),
                (case != "missing-agent").then_some(row)
            );
        }
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn report_capture_builder_runs_only_after_preflight_and_keeps_refusal_typed() {
    let (client, name, store) = database().await;
    let (agent, _) = seed(&store, 0).await;
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = client.start_session().await.unwrap();
    for invalid in [false, true] {
        let mut row = agent_document(&agent).unwrap();
        if invalid {
            row.insert("last_seq", -1_i64);
        }
        store
            .agents
            .replace_one(doc! {"_id":agent.id.as_str()}, row)
            .await
            .unwrap();
        let prepared = AtomicBool::new(false);
        let work =
            assessment::scan_report_only_prepared_admitted(
                ScanCaller::Other,
                input(&store, &agent, u64::MAX),
                &mut session,
                false,
                &policy,
                plan(),
                || {
                    prepared.store(true, Ordering::Relaxed);
                    Err::<
                        fn(SharedOwned<AgentLogEntry>) -> Result<(), OwnedWork<SandboxStoreError>>,
                        _,
                    >(
                        AdmissionRefusal::AtCapacity {
                            phase: "capture builder fixture",
                        }
                        .into(),
                    )
                },
            )
            .unwrap();
        let result = work.await;
        assert_eq!(prepared.load(Ordering::Relaxed), !invalid);
        if invalid {
            assert!(matches!(
                &*result.unwrap().transpose().unwrap_err(),
                ReadFailure::Domain(SandboxStoreError::Corrupt(_))
            ));
        } else {
            assert!(matches!(
                result,
                Err(AdmittedError::Refused(AdmissionRefusal::AtCapacity {
                    phase: "capture builder fixture"
                }))
            ));
        }
        assert_eq!(root.available_bytes(), idle);
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn report_capture_keeps_lineage_partial_failure_and_independent_results() {
    let (client, name, mut store) = database().await;
    let (agent, rows) = seed(&store, 200).await;
    let (reference_root, reference_policy) = policy();
    let (root, policy) = policy();
    let idle = root.available_bytes();
    for enabled in [false, true] {
        store = store.with_scan_cooperation(enabled);
        for case in [
            "valid",
            "missing-agent",
            "workspace",
            "late-created",
            "missing",
            "extra",
            "malformed",
        ] {
            let mut documents = rows.clone();
            match case {
                "workspace" | "missing-agent" | "late-created" => {
                    let mut entry = read_entry(&documents[0]).unwrap();
                    if case == "late-created" {
                        entry.seq = 150;
                    } else if let AgentLogKind::Created { workspace_id, .. } = &mut entry.kind {
                        workspace_id.0 = "wks_capture_other".into();
                    }
                    documents[if case == "late-created" { 149 } else { 0 }] =
                        log_document(&entry).unwrap();
                }
                "missing" => {
                    documents.remove(149);
                }
                "extra" => {
                    let mut entry = read_entry(&documents[199]).unwrap();
                    entry.seq = 201;
                    documents.push(log_document(&entry).unwrap());
                }
                "malformed" => {
                    documents[149].insert("record", "bad");
                }
                _ => {}
            }
            replace(&store, &agent, &documents).await;
            store
                .agents
                .replace_one(
                    doc! {"_id":agent.id.as_str()},
                    agent_document(&agent).unwrap(),
                )
                .upsert(true)
                .await
                .unwrap();
            if case == "missing-agent" {
                store
                    .agents
                    .delete_one(doc! {"_id":agent.id.as_str()})
                    .await
                    .unwrap();
            }
            let mut session = client.start_session().await.unwrap();
            begin(&store, &mut session).await;
            let expected = assessment::scan_report_only_admitted(
                ScanCaller::Other,
                input(&store, &agent, 200),
                &mut session,
                false,
                &reference_policy,
                plan(),
                |_| Ok(()),
            )
            .unwrap()
            .await;
            let actual = capture_report_only_admitted(
                ScanCaller::Other,
                input(&store, &agent, 200),
                &mut session,
                false,
                &policy,
                plan(),
            )
            .unwrap()
            .await;
            if matches!(case, "valid" | "missing-agent") {
                let actual = actual.unwrap().transpose().unwrap();
                let summary = expected.unwrap().transpose().unwrap();
                assert_eq!(*actual.summary, *summary);
                drop(summary);
                for (entry, row) in actual.entries.as_slice().iter().zip(&documents) {
                    assert_eq!(**entry, read_entry(row).unwrap());
                }
                let kept = actual.entries.as_slice()[149].clone();
                drop(actual);
                assert_eq!(idle - root.available_bytes(), kept.requested_bytes());
                drop(kept);
            } else {
                compare(expected, actual);
            }
            assert_open_transaction(&store, &mut session, "capture-report-marker").await;
            session.abort_transaction().await.unwrap();
            assert_eq!(root.available_bytes(), idle);
            assert_eq!(reference_root.available_bytes(), idle);
        }
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn report_capture_preserves_snapshot_replacement_checkpoint_cancellation_and_pressure() {
    let (client, name, mut store) = database().await;
    let (agent, rows) = seed(&store, 300).await;
    store = store.with_scan_cooperation(true);
    store.scan_cooperation.section_budget = std::time::Duration::ZERO;
    let (root, policy) = policy();
    let idle = root.available_bytes();
    for snapshot in [false, true] {
        for mode in ["complete", "cancel", "pressure"] {
            store
                .agents
                .replace_one(
                    doc! {"_id":agent.id.as_str()},
                    agent_document(&agent).unwrap(),
                )
                .await
                .unwrap();
            replace(&store, &agent, &rows).await;
            let mut session = client.start_session().snapshot(snapshot).await.unwrap();
            if !snapshot {
                begin(&store, &mut session).await;
            }
            let before = policy.budget(IngressClass::Data).available_bytes();
            let fresh = store
                .integrity_agent_read_admitted(&agent.id, &mut session, snapshot, &policy, plan())
                .unwrap()
                .await
                .unwrap();
            let fresh_bytes = before - policy.budget(IngressClass::Data).available_bytes();
            drop(fresh);
            let pause_bytes = fresh_bytes
                + CapturedEntries::plan(300).unwrap().requested_bytes()
                + size_of::<LogSummary>()
                + shared_owner_bytes::<()>()
                + rows[..8]
                    .iter()
                    .map(|r| owned(&read_entry(r).unwrap(), &policy).requested_bytes())
                    .sum::<usize>();
            isolated_test(async {
                let mut work = capture_report_only_admitted(
                    ScanCaller::Other,
                    input(&store, &agent, 300),
                    &mut session,
                    snapshot,
                    &policy,
                    plan(),
                )
                .unwrap();
                poll_fn(|cx| {
                    assert!(Pin::new(&mut work).poll(cx).is_pending());
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
                let mut changed = agent_document(&agent).unwrap();
                changed.insert("last_seq", -1_i64);
                store
                    .agents
                    .replace_one(doc! {"_id":agent.id.as_str()}, changed)
                    .await
                    .unwrap();
                store
                    .agent_log
                    .delete_many(doc! {"agent_id":agent.id.as_str()})
                    .await
                    .unwrap();
                match mode {
                    "complete" => {
                        let actual = work.await.unwrap().transpose().unwrap();
                        assert_eq!(actual.entries.as_slice().len(), 300);
                        for (entry, row) in actual.entries.as_slice().iter().zip(&rows) {
                            assert_eq!(**entry, read_entry(row).unwrap());
                        }
                        assert_eq!(counts()[1][..2], [1, 300]);
                    }
                    "pressure" => {
                        let hold = policy
                            .admit(
                                IngressClass::Data,
                                AllocationPlan::checked(
                                    "mid-report pressure",
                                    &[policy.budget(IngressClass::Data).available_bytes()],
                                )
                                .unwrap(),
                            )
                            .unwrap();
                        assert!(matches!(work.await, Err(AdmittedError::Refused(_))));
                        drop(hold);
                        assert_eq!(counts()[0][..2], [1, 9]);
                    }
                    _ => {
                        drop(work);
                        assert_eq!(counts()[0][..2], [1, 8]);
                    }
                }
            })
            .await;
            assert_eq!(root.available_bytes(), idle);
            if !snapshot {
                assert_open_transaction(&store, &mut session, "capture-report-marker").await;
                session.abort_transaction().await.unwrap();
            } else {
                assert!(session.snapshot_time().is_some());
            }
        }
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn report_capture_keeps_unpolled_refusal_and_original_driver_error_ownership() {
    let (client, name, store) = database().await;
    let (agent, _) = seed(&store, 200).await;
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = client.start_session().await.unwrap();
    drop(
        capture_report_only_admitted(
            ScanCaller::Other,
            input(&store, &agent, 200),
            &mut session,
            false,
            &policy,
            plan(),
        )
        .unwrap(),
    );
    assert_eq!(root.available_bytes(), idle);
    for class in [IngressClass::Data, IngressClass::Transformation] {
        let hold = policy
            .admit(
                class,
                AllocationPlan::checked("pressure", &[policy.budget(class).available_bytes()])
                    .unwrap(),
            )
            .unwrap();
        match capture_report_only_admitted(
            ScanCaller::Other,
            input(&store, &agent, 200),
            &mut session,
            false,
            &policy,
            plan(),
        ) {
            Err(e) => assert!(matches!(e, AdmittedError::Refused(_))),
            Ok(work) => assert!(matches!(work.await, Err(AdmittedError::Refused(_)))),
        }
        drop(hold);
        assert_eq!(root.available_bytes(), idle);
    }
    let mut config =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    config.app_name = Some("cma-captured-report-error".into());
    let reader = mongodb::Client::with_options(config).unwrap();
    let store = MongoSandboxStore::open(&reader, &name).await.unwrap();
    let admin = client.database("admin");
    for command in ["find", "getMore"] {
        let mut session = reader.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        admin.run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},"data":{"failCommands":[command],"appName":"cma-captured-report-error","errorCode":112,"errorLabels":["TransientTransactionError"]}}).await.unwrap();
        let error = capture_report_only_admitted(
            ScanCaller::Other,
            input(&store, &agent, 200),
            &mut session,
            false,
            &policy,
            plan(),
        )
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap_err();
        admin
            .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
            .await
            .unwrap();
        let ReadFailure::Driver(driver) = &*error else {
            panic!("original driver error")
        };
        assert!(driver.contains_label("TransientTransactionError"));
        let pointer = std::ptr::from_ref(driver.kind.as_ref());
        let moved = error.map(|e| match e {
            ReadFailure::Driver(d) => d,
            _ => unreachable!(),
        });
        assert_eq!(std::ptr::from_ref(moved.kind.as_ref()), pointer);
        assert!(moved.belongs_to(&policy));
        drop(moved);
        assert_eq!(root.available_bytes(), idle);
        let _ = session.abort_transaction().await;
    }
    reader.shutdown().await;
    client.database(&name).drop().await.unwrap();
}
