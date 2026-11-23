use super::super::fixtures::plan;
use super::super::report_fixtures::*;
use super::super::*;
use crate::store::agent_log_integrity::proof::{
    ingress::assert_open_transaction, summary_tests::policy,
};
use crate::store::agent_log_integrity::read::ingress::{
    scan_complete_log_with_session_admitted, scan_complete_log_with_session_prepared_admitted,
};
use crate::store::agent_log_telemetry::isolated_test;
use crate::store::agents::agent_document;
use crate::store::sandbox_maintenance::mongo_integration_tests::database;
use crate::store::sandbox_mongo::start_transaction;
use layer2_core::sandbox::AgentLogKind;
use std::sync::atomic::{AtomicBool, Ordering};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn captured_transaction_preserves_agent_equality_proof_and_head_precedence() {
    let (client, name, mut store) = database().await;
    let (original, _) = seed(&store, 0).await;
    let (reference_root, reference_policy) = policy();
    let (root, policy) = policy();
    let idle = root.available_bytes();
    for percent in [0, 100] {
        store = store.with_fleet_log_percentage(percent);
        for case in [
            "valid",
            "absent",
            "missing-agent",
            "conflict",
            "quarantine",
            "identity",
            "malformed",
            "segmented",
            "bad-agent",
            "unsupported",
        ] {
            for head in [0, 1, PHYSICAL_AGENT_LOG_ENTRIES + 1] {
                let mut caller = original.clone();
                caller.conversation.last_seq = head;
                let row = report_row(&caller, case);
                store
                    .agents
                    .replace_one(doc! {"_id":caller.id.as_str()}, row.clone())
                    .upsert(true)
                    .await
                    .unwrap();
                if case == "missing-agent" {
                    store
                        .agents
                        .delete_one(doc! {"_id":caller.id.as_str()})
                        .await
                        .unwrap();
                } else if case == "conflict" {
                    caller.revision += 1;
                }
                let mut session = client.start_session().await.unwrap();
                begin(&store, &mut session).await;
                let (expected, metrics) = isolated_test(async {
                    let value = scan_complete_log_with_session_admitted(
                        ScanCaller::Other,
                        &store,
                        &caller,
                        &mut session,
                        &reference_policy,
                        plan(),
                    )
                    .unwrap()
                    .await;
                    (value, counts())
                })
                .await;
                isolated_test(async {
                    let actual = capture_complete_log_with_session_admitted(
                        ScanCaller::Other,
                        &store,
                        &caller,
                        &mut session,
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
                assert_eq!(
                    store
                        .agents
                        .find_one(doc! {"_id":caller.id.as_str()})
                        .await
                        .unwrap(),
                    (case != "missing-agent").then_some(row)
                );
            }
        }
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn transaction_capture_builder_follows_all_preflight_and_refuses_without_corruption() {
    let (client, name, store) = database().await;
    let (agent, _) = seed(&store, 0).await;
    let (root, policy) = policy();
    let idle = root.available_bytes();
    for case in [
        "valid",
        "bad-agent",
        "conflict",
        "missing-agent",
        "malformed",
        "oversized",
    ] {
        let mut caller = agent.clone();
        if case == "oversized" {
            caller.conversation.last_seq = PHYSICAL_AGENT_LOG_ENTRIES + 1;
        }
        let row = report_row(&caller, if case == "oversized" { "absent" } else { case });
        store
            .agents
            .replace_one(doc! {"_id":caller.id.as_str()}, row)
            .upsert(true)
            .await
            .unwrap();
        if case == "conflict" {
            caller.revision += 1;
        } else if case == "missing-agent" {
            store
                .agents
                .delete_one(doc! {"_id":caller.id.as_str()})
                .await
                .unwrap();
        }
        let prepared = AtomicBool::new(false);
        let mut session = client.start_session().await.unwrap();
        begin(&store, &mut session).await;
        let result =
            scan_complete_log_with_session_prepared_admitted(
                ScanCaller::Other,
                &store,
                &caller,
                &mut session,
                &policy,
                plan(),
                || {
                    prepared.store(true, Ordering::Relaxed);
                    Err::<
                        fn(SharedOwned<AgentLogEntry>) -> Result<(), OwnedWork<SandboxStoreError>>,
                        _,
                    >(
                        AdmissionRefusal::AtCapacity {
                            phase: "transaction capture builder fixture",
                        }
                        .into(),
                    )
                },
            )
            .unwrap()
            .await;
        let should_prepare = matches!(case, "valid" | "oversized");
        assert_eq!(prepared.load(Ordering::Relaxed), should_prepare, "{case}");
        if should_prepare {
            assert!(matches!(
                result,
                Err(AdmittedError::Refused(AdmissionRefusal::AtCapacity {
                    phase: "transaction capture builder fixture"
                }))
            ));
        } else {
            let error = result.unwrap().transpose().unwrap_err();
            assert!(matches!(
                (&*error, case),
                (ReadFailure::Domain(SandboxStoreError::Conflict), "conflict")
                    | (
                        ReadFailure::Domain(SandboxStoreError::NotFound),
                        "missing-agent"
                    )
                    | (
                        ReadFailure::Domain(SandboxStoreError::Corrupt(_)),
                        "bad-agent" | "malformed"
                    )
            ));
        }
        assert_open_transaction(&store, &mut session, "capture-report-marker").await;
        session.abort_transaction().await.unwrap();
        assert_eq!(root.available_bytes(), idle);
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn transaction_capture_keeps_lineage_partial_failure_and_independent_results() {
    let (client, name, mut store) = database().await;
    let (agent, rows) = seed(&store, 200).await;
    let (reference_root, reference_policy) = policy();
    let (root, policy) = policy();
    let idle = root.available_bytes();
    for enabled in [false, true] {
        store = store.with_scan_cooperation(enabled);
        for case in [
            "valid",
            "workspace",
            "late-created",
            "missing",
            "extra",
            "malformed",
        ] {
            let mut documents = rows.clone();
            match case {
                "workspace" | "late-created" => {
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
            let mut session = client.start_session().await.unwrap();
            begin(&store, &mut session).await;
            let expected = scan_complete_log_with_session_admitted(
                ScanCaller::Other,
                &store,
                &agent,
                &mut session,
                &reference_policy,
                plan(),
            )
            .unwrap()
            .await;
            let actual = capture_complete_log_with_session_admitted(
                ScanCaller::Other,
                &store,
                &agent,
                &mut session,
                &policy,
                plan(),
            )
            .unwrap()
            .await;
            if case == "valid" {
                let actual = actual.unwrap().transpose().unwrap();
                let summary = expected.unwrap().transpose().unwrap();
                assert_eq!(*actual.summary, *summary);
                drop(summary);
                assert_eq!(actual.entries.as_slice().len(), documents.len());
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

#[path = "scan_capture_transaction_lifecycle_tests.rs"]
mod lifecycle;
