use super::*;
use crate::store::agent_log_telemetry::{isolated_test, recorded_execution, snapshot};
use layer2_core::sandbox::AgentLogKind;
use std::{future::Future, pin::Pin};

fn validate<'a>(
    store: &'a MongoSandboxStore,
    agent: &'a Agent,
    raw: &'a mongodb::bson::RawDocument,
    session: &'a mut mongodb::ClientSession,
    snapshot: bool,
    policy: &'a IngressOwnershipPolicy,
) -> Result<SummaryRead<'a>, AdmittedError> {
    validate_summary_admitted(
        store,
        ProofInput { agent, raw },
        ScanCaller::Other,
        session,
        snapshot,
        policy,
        plan(),
    )
}

fn corrupt_row(agent: &Agent) -> Document {
    let mut proof = metadata(agent, [0; HASH_BYTES]);
    proof.history_identity = [1; HASH_BYTES];
    doc! {INTEGRITY_FIELD:proof.to_document().unwrap()}
}

fn rows(agent: &Agent) -> Vec<Document> {
    (1..=agent.conversation.last_seq)
        .map(|seq| {
            log_document(&AgentLogEntry {
                agent_id: agent.id.clone(),
                seq,
                appended_at: "2026-01-01T00:00:00Z".into(),
                kind: AgentLogKind::AgentMessage {
                    content: "x".repeat(4096),
                },
            })
            .unwrap()
        })
        .collect()
}

fn compare(
    expected: Result<Option<LogSummary>, TransactionLogError>,
    actual: OwnedWork<Result<Option<SharedOwned<LogSummary>>, ReadFailure>>,
    policy: &IngressOwnershipPolicy,
) {
    assert!(actual.belongs_to(policy));
    match (expected, actual.transpose()) {
        (Ok(expected), Ok(actual)) => assert_eq!(expected.as_ref(), actual.as_ref().map(|s| &**s)),
        (Err(TransactionLogError::Domain(expected)), Err(actual)) => {
            assert!(matches!(&*actual, ReadFailure::Domain(error) if *error == expected));
        }
        other => panic!("proof validation changed: {other:?}"),
    }
}

fn failed_counts() -> [u64; 3] {
    let counts = snapshot(ScanCaller::Other, false);
    assert_eq!(snapshot(ScanCaller::Other, true)[0], 0);
    [counts[0], counts[1], counts[2]]
}

fn pressure(
    policy: &IngressOwnershipPolicy,
    class: IngressClass,
    remaining: usize,
) -> layer2_core::sandbox::ingress_ownership::ReservationBundle {
    policy
        .admit(
            class,
            AllocationPlan::checked(
                "proof pressure",
                &[policy.budget(class).available_bytes() - remaining],
            )
            .unwrap(),
        )
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn proof_validation_matches_original_and_leaves_transaction_and_quarantine_to_caller() {
    let (client, name, store) = database().await;
    let agent = empty_agent(&store, "sbx_validate_matrix").await;
    let original = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap();
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    store
        .agent_log
        .insert_one(doc! {"_id":"validation-marker"})
        .session(&mut session)
        .await
        .unwrap();
    for case in [
        "valid",
        "absent",
        "stale",
        "invalid",
        "quarantine-false",
        "quarantine-null",
        "malformed",
        "identity",
    ] {
        let mut block = metadata(&agent, [0; HASH_BYTES]);
        block.bound_agent_revision -= u64::from(case == "stale");
        block.semantic.valid = case != "invalid";
        let mut row = doc! {INTEGRITY_FIELD:block.to_document().unwrap()};
        match case {
            "absent" => {
                row.remove(INTEGRITY_FIELD);
            }
            "quarantine-false" => {
                row.insert(QUARANTINE_FIELD, false);
            }
            "quarantine-null" => {
                row.insert(QUARANTINE_FIELD, mongodb::bson::Bson::Null);
            }
            "malformed" => {
                row.insert(INTEGRITY_FIELD, "bad");
            }
            "identity" => {
                row = corrupt_row(&agent);
            }
            _ => {}
        }
        let raw = RawDocumentBuf::try_from(&row).unwrap();
        let (expected, counts) = isolated_test(async {
            let result = proof::validate_summary(
                &store,
                &agent,
                &row,
                ScanCaller::Other,
                &mut session,
                false,
            )
            .await;
            (result, failed_counts())
        })
        .await;
        isolated_test(async {
            let actual = validate(&store, &agent, &raw, &mut session, false, &policy)
                .unwrap()
                .await
                .unwrap();
            compare(expected, actual, &policy);
            assert_eq!(failed_counts(), counts, "{case}");
            assert_eq!(
                counts[0],
                u64::from(matches!(case, "malformed" | "identity"))
            );
        })
        .await;
        assert_eq!(root.available_bytes(), idle);
        assert_open_transaction(&store, &mut session, "validation-marker").await;
    }
    session.abort_transaction().await.unwrap();
    assert_eq!(
        store
            .agents
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap(),
        original
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn known_corruption_keeps_primary_error_and_failed_scan_accounting_in_both_cursor_modes() {
    let (client, name, mut store) = database().await;
    let mut agent = empty_agent(&store, "sbx_validate_scan").await;
    agent.conversation.last_seq = 200;
    let documents = rows(&agent);
    let (root, policy) = policy();
    let idle = root.available_bytes();
    for cooperation in [false, true] {
        store = store.with_scan_cooperation(cooperation);
        for case in [
            "identity",
            "hash",
            "missing",
            "malformed",
            "unsupported",
            "refused",
        ] {
            store
                .agent_log
                .delete_many(doc! {"agent_id":agent.id.as_str()})
                .await
                .unwrap();
            let mut stored = documents.clone();
            match case {
                "missing" => {
                    stored.remove(99);
                }
                "malformed" => {
                    stored[99].insert("record", "bad");
                }
                "refused" => {
                    stored[0].insert("extra", "x".repeat(512 * 1024));
                }
                "unsupported" => {
                    let mut nested = doc! {"leaf":true};
                    for _ in 0..66 {
                        nested = doc! {"nested":nested};
                    }
                    stored[99].insert("unused", nested);
                }
                _ => {}
            }
            store.agent_log.insert_many(stored.clone()).await.unwrap();
            let row = if case == "hash" {
                summary_row(&agent)
            } else {
                corrupt_row(&agent)
            };
            let raw = RawDocumentBuf::try_from(&row).unwrap();
            let mut session = client.start_session().snapshot(true).await.unwrap();
            let (expected, counts) = isolated_test(async {
                let result = proof::validate_summary(
                    &store,
                    &agent,
                    &row,
                    ScanCaller::Other,
                    &mut session,
                    true,
                )
                .await;
                (result, failed_counts())
            })
            .await;
            let timestamp = session.snapshot_time();
            let pressure =
                (case == "refused").then(|| pressure(&policy, IngressClass::Data, 256 * 1024));
            let (_, execution) = recorded_execution(isolated_test(async {
                let actual = validate(&store, &agent, &raw, &mut session, true, &policy)
                    .unwrap()
                    .await
                    .unwrap();
                compare(expected, actual, &policy);
                let actual = failed_counts();
                assert_eq!(actual[0], 1);
                if !matches!(case, "unsupported" | "refused") {
                    assert_eq!(actual, counts);
                }
                let read = if case == "refused" {
                    1
                } else if matches!(case, "missing" | "malformed" | "unsupported") {
                    100
                } else {
                    200
                };
                assert_eq!(actual[1], read);
                assert_eq!(
                    actual[2],
                    stored
                        .iter()
                        .take(read as usize)
                        .map(|d| d.to_vec().unwrap().len() as u64)
                        .sum::<u64>()
                );
            }))
            .await;
            assert_eq!(execution.len(), 1);
            if cooperation && case != "refused" {
                assert!(execution[0].1 > 0);
            }
            drop(pressure);
            assert_eq!(session.snapshot_time(), timestamp);
            assert_eq!(root.available_bytes(), idle);
        }
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn proof_admission_refusal_skips_diagnostics_and_secondary_refusal_keeps_corruption() {
    let (client, name, store) = database().await;
    let agent = empty_agent(&store, "sbx_validate_refusal").await;
    let raw = RawDocumentBuf::try_from(&corrupt_row(&agent)).unwrap();
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    store
        .agent_log
        .insert_one(doc! {"_id":"refusal-marker"})
        .session(&mut session)
        .await
        .unwrap();
    drop(validate(&store, &agent, &raw, &mut session, false, &policy).unwrap());
    assert_eq!(root.available_bytes(), idle);
    for class in [IngressClass::Data, IngressClass::Transformation] {
        let pressure = pressure(&policy, class, 0);
        isolated_test(async {
            match validate(&store, &agent, &raw, &mut session, false, &policy) {
                Ok(work) => assert!(matches!(work.await, Err(AdmittedError::Refused(_)))),
                Err(error) => assert!(matches!(error, AdmittedError::Refused(_))),
            }
            assert_eq!(failed_counts(), [0; 3]);
        })
        .await;
        drop(pressure);
    }
    let mut deep = corrupt_row(&agent);
    let mut nested = doc! {"leaf":true};
    for _ in 0..66 {
        nested = doc! {"nested":nested};
    }
    deep.insert("unused", nested);
    let deep = RawDocumentBuf::try_from(&deep).unwrap();
    isolated_test(async {
        assert!(matches!(
            validate(&store, &agent, &deep, &mut session, false, &policy)
                .unwrap()
                .await,
            Err(AdmittedError::Unsupported(_))
        ));
        assert_eq!(failed_counts(), [0; 3]);
    })
    .await;
    let pressure = pressure(&policy, IngressClass::Transformation, 1024 * 1024);
    isolated_test(async {
        let expected =
            proof::read_only_summary(&store, &agent, &corrupt_row(&agent), &mut session, false)
                .await;
        let actual = validate(&store, &agent, &raw, &mut session, false, &policy)
            .unwrap()
            .await
            .unwrap();
        compare(expected, actual, &policy);
        assert_eq!(failed_counts(), [0; 3]);
    })
    .await;
    drop(pressure);
    assert_open_transaction(&store, &mut session, "refusal-marker").await;
    session.abort_transaction().await.unwrap();
    assert_eq!(root.available_bytes(), idle);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn diagnostic_pending_polls_keep_primary_custody_and_pinned_snapshot() {
    let (client, name, store) = database().await;
    let mut agent = empty_agent(&store, "sbx_validate_custody").await;
    agent.conversation.last_seq = 200;
    store.agent_log.insert_many(rows(&agent)).await.unwrap();
    let raw = RawDocumentBuf::try_from(&corrupt_row(&agent)).unwrap();
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = client.start_session().snapshot(true).await.unwrap();
    store
        .agent_log
        .find_one(doc! {"agent_id":agent.id.as_str()})
        .read_concern(mongodb::options::ReadConcern::snapshot())
        .session(&mut session)
        .await
        .unwrap();
    let timestamp = session.snapshot_time().expect("pinned snapshot");
    store
        .agent_log
        .delete_many(doc! {"agent_id":agent.id.as_str()})
        .await
        .unwrap();
    let primary =
        read_only_summary_admitted(&store, &agent, &raw, &mut session, true, &policy, plan())
            .unwrap()
            .await
            .unwrap();
    let primary_bytes = idle - root.available_bytes();
    assert!(matches!(
        &*primary,
        Err(ReadFailure::Domain(SandboxStoreError::Corrupt(_)))
    ));
    drop(primary);
    let scan = scan::cursor::scan_corruption_admitted(
        ScanCaller::Other,
        scan::cursor::ScanInput {
            collection: &store.agent_log,
            agent_id: &agent.id,
            last_seq: agent.conversation.last_seq,
        },
        &mut session,
        ScanOptions::new(true, store.scan_cooperation),
        &policy,
        plan(),
    )
    .unwrap();
    let scan_bytes = scan.requested_bytes();
    drop(scan);
    isolated_test(async {
        let mut work = validate(&store, &agent, &raw, &mut session, true, &policy).unwrap();
        let expected = work.requested_bytes() + scan_bytes + primary_bytes;
        let mut pending = 0;
        let result = std::future::poll_fn(|cx| {
            let result = Pin::new(&mut work).poll(cx);
            if result.is_pending() {
                pending += 1;
                assert_eq!(
                    policy.bounds().transformation_bytes()
                        - policy
                            .budget(IngressClass::Transformation)
                            .available_bytes(),
                    expected
                );
            }
            result
        })
        .await
        .unwrap();
        assert!(pending > 0);
        drop(work);
        assert_eq!(failed_counts()[..2], [1, 200]);
        assert_eq!(idle - root.available_bytes(), primary_bytes);
        let error = result.transpose().unwrap_err();
        let ReadFailure::Domain(SandboxStoreError::Corrupt(message)) = &*error else {
            panic!("primary corruption required")
        };
        let pointer = message.as_ptr();
        let moved = error.map(|error| match error {
            ReadFailure::Domain(SandboxStoreError::Corrupt(message)) => message,
            _ => unreachable!(),
        });
        assert_eq!(moved.as_ptr(), pointer);
        assert!(moved.belongs_to(&policy));
        drop(moved);
    })
    .await;
    assert_eq!(session.snapshot_time(), Some(timestamp));
    assert_eq!(root.available_bytes(), idle);
    let mut fresh = client.start_session().snapshot(true).await.unwrap();
    isolated_test(async {
        drop(
            validate(&store, &agent, &raw, &mut fresh, true, &policy)
                .unwrap()
                .await
                .unwrap(),
        );
        assert_eq!(failed_counts()[..2], [1, 0]);
    })
    .await;
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn secondary_driver_errors_do_not_replace_primary_corruption() {
    let (client, name, store) = database().await;
    let mut agent = empty_agent(&store, "sbx_validate_driver").await;
    agent.conversation.last_seq = 200;
    store.agent_log.insert_many(rows(&agent)).await.unwrap();
    let raw = RawDocumentBuf::try_from(&corrupt_row(&agent)).unwrap();
    let mut options =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some("cma-proof-diagnostic-error".into());
    let reader = mongodb::Client::with_options(options).unwrap();
    let mut store = MongoSandboxStore::open(&reader, &name).await.unwrap();
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let admin = client.database("admin");
    for cooperation in [false, true] {
        store = store.with_scan_cooperation(cooperation);
        for command in ["find", "getMore"] {
            let mut session = reader.start_session().await.unwrap();
            start_transaction(&mut session).await.unwrap();
            admin.run_command(doc! {"configureFailPoint":"failCommand", "mode":{"times":1}, "data":{
                "failCommands":[command], "appName":"cma-proof-diagnostic-error", "errorCode":112, "errorLabels":["TransientTransactionError"]
            }}).await.unwrap();
            isolated_test(async {
                let result = validate(&store, &agent, &raw, &mut session, false, &policy).unwrap().await.unwrap();
                assert!(matches!(&*result, Err(ReadFailure::Domain(SandboxStoreError::Corrupt(message))) if message == "Agent log integrity metadata is corrupt"));
                assert_eq!(failed_counts()[..2], [1, if command == "find" { 0 } else { 101 }]);
            }).await;
            admin
                .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
                .await
                .unwrap();
            let _ = session.abort_transaction().await;
            assert_eq!(root.available_bytes(), idle);
        }
    }
    reader.shutdown().await;
    client.database(&name).drop().await.unwrap();
}
