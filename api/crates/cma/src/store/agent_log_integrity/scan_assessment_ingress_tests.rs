use super::*;
use crate::store::agent_log_integrity::proof::{
    ingress::assert_open_transaction,
    summary_tests::{metadata, policy},
};
use crate::store::agent_log_telemetry::isolated_test;
use crate::store::agents::agent_document;
use crate::store::sandbox_maintenance::mongo_integration_tests::database;
use crate::store::sandbox_mongo::start_transaction;
use layer2_core::sandbox::{ingress_ownership::IngressWorkScope, AgentLogKind};
use std::{
    future::{poll_fn, Future},
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::Poll,
};

use super::fixtures::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn report_only_keeps_optional_agent_format_proof_and_caller_head_semantics() {
    let (client, name, mut store) = database().await;
    let (agent, _) = seed(&store, "sbx_report_matrix", 0).await;
    let (root, policy) = policy();
    let idle = root.available_bytes();
    for percent in [0, 100] {
        store = store.with_fleet_log_percentage(percent);
        for case in [
            "valid",
            "absent",
            "missing-agent",
            "caller-head",
            "quarantine",
            "identity",
            "malformed",
            "segmented",
            "bad-agent",
            "unsupported",
        ] {
            let mut row = agent_document(&agent).unwrap();
            let mut proof = metadata(&agent, [0; HASH_BYTES]);
            if case == "identity" {
                proof.history_identity = [1; HASH_BYTES];
            }
            row.insert(INTEGRITY_FIELD, proof.to_document().unwrap());
            match case {
                "absent" => {
                    row.remove(INTEGRITY_FIELD);
                }
                "quarantine" => {
                    row.insert(QUARANTINE_FIELD, false);
                }
                "malformed" => {
                    row.insert(INTEGRITY_FIELD, "bad");
                }
                "segmented" => {
                    row.insert("document_schema_version", 2_i64);
                    row.insert("agent_log_format", "segmented_v1");
                    row.insert(
                        "agent_log_incarnation",
                        crate::store::agent_log_segments::incarnation(&agent),
                    );
                    row.insert("agent_log_head_revision", 1_i64);
                }
                "bad-agent" => {
                    row.insert("last_seq", -1_i64);
                }
                "unsupported" => {
                    let mut nested = doc! {"leaf":true};
                    for _ in 0..66 {
                        nested = doc! {"nested":nested};
                    }
                    row.insert("unused", nested);
                }
                _ => {}
            }
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
            let mut caller = agent.clone();
            if case == "caller-head" {
                caller.conversation.last_seq = 1;
            }
            let mut session = client.start_session().await.unwrap();
            begin(&store, &mut session).await;
            let (expected, expected_counts) = isolated_test(async {
                let result = legacy(&store, &caller, &mut session, false, |_| Ok(())).await;
                (result, counts())
            })
            .await;
            isolated_test(async {
                let result = assess(&store, &caller, &mut session, false, &policy, |_| Ok(()))
                    .unwrap()
                    .await;
                if case == "unsupported" {
                    assert!(expected.is_ok());
                    assert!(matches!(result, Err(AdmittedError::Unsupported(_))));
                    assert_eq!(counts(), [[0; 3]; 2]);
                } else {
                    assert_eq!(
                        expected.is_ok(),
                        matches!(case, "valid" | "absent" | "missing-agent")
                    );
                    compare(expected, result.unwrap(), &policy);
                    assert_eq!(counts(), expected_counts, "{case}");
                }
            })
            .await;
            assert_open_transaction(&store, &mut session, "report-marker").await;
            session.abort_transaction().await.unwrap();
            assert_eq!(
                store
                    .agents
                    .find_one(doc! {"_id":agent.id.as_str()})
                    .await
                    .unwrap(),
                (case != "missing-agent").then_some(row)
            );
            assert_eq!(root.available_bytes(), idle);
        }
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn report_only_preserves_complete_scan_lineage_and_optional_agent_visitors() {
    let (client, name, mut store) = database().await;
    store.agent_log = client.database(&name).collection("report_corrupt_history");
    let (agent, documents) = seed(&store, "sbx_report_scan", 300).await;
    assert!(
        documents
            .iter()
            .map(|d| d.to_vec().unwrap().len())
            .sum::<usize>()
            > 4 * 1024 * 1024
    );
    let (root, policy) = policy();
    let idle = root.available_bytes();
    for cooperation in [false, true] {
        store = store.with_scan_cooperation(cooperation);
        for case in [
            "valid",
            "missing",
            "duplicate",
            "extra",
            "malformed",
            "workspace",
            "worktree",
            "late-created",
            "missing-agent",
        ] {
            let mut stored = documents.clone();
            match case {
                "missing" => {
                    stored.remove(149);
                }
                "duplicate" => {
                    let mut row = stored[149].clone();
                    row.insert("_id", "report-duplicate");
                    stored.push(row);
                }
                "extra" => {
                    let mut entry = read_entry(&stored[299]).unwrap();
                    entry.seq = 301;
                    stored.push(log_document(&entry).unwrap());
                }
                "malformed" => {
                    stored[149].insert("record", "bad");
                }
                "workspace" | "worktree" | "late-created" | "missing-agent" => {
                    let mut entry = read_entry(&stored[0]).unwrap();
                    if case == "late-created" {
                        entry.seq = 150;
                    }
                    if let AgentLogKind::Created {
                        workspace_id,
                        worktree_id,
                    } = &mut entry.kind
                    {
                        if matches!(case, "workspace" | "missing-agent") {
                            workspace_id.0 = "wks_report_other".into();
                        }
                        if case == "worktree" {
                            worktree_id.0 = "wkt_report_other".into();
                        }
                    }
                    stored[if case == "late-created" { 149 } else { 0 }] =
                        log_document(&entry).unwrap();
                }
                _ => {}
            }
            replace_rows(&store, &agent, &stored).await;
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
            let mut expected_visits = Vec::new();
            let expected = legacy(&store, &agent, &mut session, false, |entry| {
                expected_visits.push(entry.seq);
                Ok(())
            })
            .await;
            assert_eq!(expected.is_ok(), matches!(case, "valid" | "missing-agent"));
            let mut actual_visits = Vec::new();
            let actual = assess(&store, &agent, &mut session, false, &policy, |entry| {
                actual_visits.push(entry.seq);
                Ok(())
            })
            .unwrap()
            .await
            .unwrap();
            compare(expected, actual, &policy);
            assert_eq!(actual_visits, expected_visits);
            assert_open_transaction(&store, &mut session, "report-marker").await;
            session.abort_transaction().await.unwrap();
            assert_eq!(root.available_bytes(), idle);
        }
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn report_only_retains_summary_entries_and_original_visitor_error_independently() {
    let (client, name, store) = database().await;
    let (agent, _) = seed(&store, "sbx_report_retention", 200).await;
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = client.start_session().await.unwrap();
    begin(&store, &mut session).await;
    let mut retained = None;
    let summary = assess(&store, &agent, &mut session, false, &policy, |entry| {
        if entry.seq == 8 {
            retained = Some(entry);
        }
        Ok(())
    })
    .unwrap()
    .await
    .unwrap()
    .transpose()
    .unwrap();
    assert_eq!(summary.entry_count(), 200);
    assert!(retained.as_ref().unwrap().belongs_to(&policy));
    drop(summary);
    assert_eq!(
        idle - root.available_bytes(),
        retained.as_ref().unwrap().requested_bytes()
    );
    drop(retained.take());
    let scope = IngressWorkScope::new(
        policy
            .admit(
                IngressClass::Transformation,
                AllocationPlan::checked("visitor error", &[4096]).unwrap(),
            )
            .unwrap(),
    );
    let mut original =
        Some(scope.initialize_owned(|| SandboxStoreError::Corrupt("report visitor".repeat(8))));
    let SandboxStoreError::Corrupt(message) = &**original.as_ref().unwrap() else {
        unreachable!()
    };
    let pointer = message.as_ptr();
    drop(scope);
    let mut visits = 0;
    let error = isolated_test(async {
        let result = assess(&store, &agent, &mut session, false, &policy, |entry| {
            visits += 1;
            if visits == 8 {
                retained = Some(entry);
                Err(original.take().unwrap())
            } else {
                Ok(())
            }
        })
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap_err();
        assert_eq!(counts()[0][..2], [1, 8]);
        assert_eq!(counts()[1][0], 0);
        result
    })
    .await;
    assert_eq!(visits, 8);
    assert!(
        matches!(&*error, ReadFailure::Domain(SandboxStoreError::Corrupt(message)) if message.as_ptr() == pointer)
    );
    drop(error);
    assert_eq!(
        idle - root.available_bytes(),
        retained.as_ref().unwrap().requested_bytes()
    );
    drop(retained);
    assert_eq!(root.available_bytes(), idle);
    assert_open_transaction(&store, &mut session, "report-marker").await;
    session.abort_transaction().await.unwrap();
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn report_only_keeps_snapshot_and_transaction_through_replacement_and_cancellation() {
    let (client, name, mut store) = database().await;
    let (agent, documents) = seed(&store, "sbx_report_snapshot", 300).await;
    store = store.with_scan_cooperation(true);
    store.scan_cooperation.section_budget = std::time::Duration::ZERO;
    let (root, policy) = policy();
    let idle = root.available_bytes();
    for snapshot in [false, true] {
        for cancel in [false, true] {
            store
                .agents
                .replace_one(
                    doc! {"_id":agent.id.as_str()},
                    agent_document(&agent).unwrap(),
                )
                .await
                .unwrap();
            replace_rows(&store, &agent, &documents).await;
            let mut session = client.start_session().snapshot(snapshot).await.unwrap();
            if !snapshot {
                begin(&store, &mut session).await;
            }
            let visits = AtomicUsize::new(0);
            let mut retained = None;
            isolated_test(async {
                let mut work = assess(&store, &agent, &mut session, snapshot, &policy, |entry| {
                    visits.fetch_add(1, Ordering::Relaxed);
                    if entry.seq == 8 {
                        retained = Some(entry);
                    }
                    Ok(())
                })
                .unwrap();
                poll_fn(|cx| {
                    assert!(Pin::new(&mut work).poll(cx).is_pending());
                    if visits.load(Ordering::Relaxed) == 8 {
                        Poll::Ready(())
                    } else {
                        Poll::Pending
                    }
                })
                .await;
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
                if cancel {
                    drop(work);
                    assert_eq!(counts()[0][..2], [1, 8]);
                    assert_eq!(counts()[1][0], 0);
                } else {
                    let summary = work.await.unwrap().transpose().unwrap();
                    assert_eq!(summary.entry_count(), 300);
                    assert_eq!(visits.load(Ordering::Relaxed), 300);
                    assert_eq!(counts()[1][..2], [1, 300]);
                    drop(summary);
                }
            })
            .await;
            assert_eq!(
                idle - root.available_bytes(),
                retained.as_ref().unwrap().requested_bytes()
            );
            drop(retained);
            assert_eq!(root.available_bytes(), idle);
            if !snapshot {
                assert_open_transaction(&store, &mut session, "report-marker").await;
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
async fn report_only_preserves_admission_refusal_unpolled_drop_and_original_driver_errors() {
    let (client, name, store) = database().await;
    let (agent, _) = seed(&store, "sbx_report_errors", 200).await;
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = client.start_session().await.unwrap();
    begin(&store, &mut session).await;
    drop(assess(&store, &agent, &mut session, false, &policy, |_| Ok(())).unwrap());
    for class in [IngressClass::Transformation, IngressClass::Data] {
        let pressure = policy
            .admit(
                class,
                AllocationPlan::checked(
                    "assessment pressure",
                    &[policy.budget(class).available_bytes()],
                )
                .unwrap(),
            )
            .unwrap();
        match assess(&store, &agent, &mut session, false, &policy, |_| Ok(())) {
            Ok(work) => assert!(matches!(work.await, Err(AdmittedError::Refused(_)))),
            Err(error) => assert!(matches!(error, AdmittedError::Refused(_))),
        }
        drop(pressure);
        assert_open_transaction(&store, &mut session, "report-marker").await;
        assert_eq!(root.available_bytes(), idle);
    }
    session.abort_transaction().await.unwrap();
    let mut options =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some("cma-report-only-error".into());
    let reader = mongodb::Client::with_options(options).unwrap();
    let mut store = MongoSandboxStore::open(&reader, &name).await.unwrap();
    let admin = client.database("admin");
    for cooperation in [false, true] {
        store = store.with_scan_cooperation(cooperation);
        for command in ["find", "getMore"] {
            let mut session = reader.start_session().await.unwrap();
            start_transaction(&mut session).await.unwrap();
            admin.run_command(doc! {"configureFailPoint":"failCommand", "mode":{"times":1}, "data":{
                "failCommands":[command], "appName":"cma-report-only-error", "errorCode":112, "errorLabels":["TransientTransactionError"]
            }}).await.unwrap();
            let error = assess(&store, &agent, &mut session, false, &policy, |_| Ok(()))
                .unwrap()
                .await
                .unwrap()
                .transpose()
                .unwrap_err();
            admin
                .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
                .await
                .unwrap();
            let ReadFailure::Driver(driver) = &*error else {
                panic!("original driver required")
            };
            assert!(driver.contains_label("TransientTransactionError"));
            let pointer = std::ptr::from_ref(driver.kind.as_ref());
            let moved = error.map(|error| match error {
                ReadFailure::Driver(driver) => driver,
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
    client.database(&name).drop().await.unwrap();
}
