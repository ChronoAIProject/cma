use super::*;
use crate::store::agent_log_integrity::read::scan_complete_log_with_session;
use layer2_core::sandbox::AgentLogKind;

fn assess<'a>(
    store: &'a MongoSandboxStore,
    agent: &'a Agent,
    session: &'a mut mongodb::ClientSession,
    policy: &'a IngressOwnershipPolicy,
) -> Result<CompleteRead<'a>, AdmittedError> {
    scan_complete_log_with_session_admitted(
        ScanCaller::Other,
        store,
        agent,
        session,
        policy,
        plan(),
    )
}

fn compare(
    original: Result<LogSummary, TransactionLogError>,
    actual: OwnedWork<Result<LogSummary, ReadFailure>>,
    policy: &IngressOwnershipPolicy,
) {
    match (original, actual.transpose()) {
        (Ok(expected), Ok(actual)) => {
            assert_eq!(*actual, expected);
            assert!(actual.belongs_to(policy));
        }
        (Err(TransactionLogError::Domain(expected)), Err(actual)) => {
            assert!(matches!(&*actual, ReadFailure::Domain(error) if *error == expected));
            assert!(actual.belongs_to(policy));
        }
        other => panic!("assessment changed original outcome: {other:?}"),
    }
}

async fn begin(store: &MongoSandboxStore, session: &mut mongodb::ClientSession) {
    start_transaction(session).await.unwrap();
    store
        .agent_log
        .insert_one(doc! {"_id":"assessment-marker"})
        .session(session)
        .await
        .unwrap();
}

fn rows(agent: &Agent, count: u64, bytes: usize) -> Vec<Document> {
    (1..=count)
        .map(|seq| {
            let kind = if seq == 1 {
                AgentLogKind::Created {
                    workspace_id: agent.workspace_id.clone(),
                    worktree_id: agent.creation_worktree_id().clone(),
                }
            } else {
                AgentLogKind::AgentMessage {
                    content: "x".repeat(bytes),
                }
            };
            log_document(&AgentLogEntry {
                agent_id: agent.id.clone(),
                seq,
                appended_at: "2026-01-01T00:00:00Z".into(),
                kind,
            })
            .unwrap()
        })
        .collect()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn complete_assessment_preserves_agent_and_proof_outcomes_without_transaction_effects() {
    let (client, name, mut store) = database().await;
    let agent = empty_agent(&store, "sbx_assessment_proof").await;
    let (root, policy) = policy();
    let idle = root.available_bytes();
    for percent in [0, 100] {
        store = store.with_fleet_log_percentage(percent);
        for case in [
            "trusted",
            "absent",
            "stale",
            "invalid",
            "quarantine",
            "malformed",
            "extra",
            "conflict",
            "missing",
            "bad-agent",
            "unsupported",
        ] {
            let mut row = summary_row(&agent);
            let mut caller = agent.clone();
            match case {
                "absent" => {
                    row.remove(INTEGRITY_FIELD);
                }
                "stale" => {
                    row.get_document_mut(INTEGRITY_FIELD)
                        .unwrap()
                        .insert("bound_agent_revision", 0_i64);
                }
                "invalid" => {
                    row.get_document_mut(INTEGRITY_FIELD)
                        .unwrap()
                        .get_document_mut(SEMANTIC_FIELD)
                        .unwrap()
                        .insert("valid", false);
                }
                "quarantine" => {
                    row.insert(QUARANTINE_FIELD, false);
                }
                "malformed" => {
                    row.insert(INTEGRITY_FIELD, "invalid");
                }
                "extra" => {
                    row.remove(INTEGRITY_FIELD);
                    store
                        .agent_log
                        .insert_many(rows(&agent, 1, 0))
                        .await
                        .unwrap();
                }
                "conflict" => {
                    caller.revision += 1;
                }
                "missing" => {
                    caller.id = AgentId("agt_assessment_missing".into());
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
                .await
                .unwrap();
            let mut session = client.start_session().await.unwrap();
            begin(&store, &mut session).await;
            let original =
                scan_complete_log_with_session(ScanCaller::Other, &store, &caller, &mut session)
                    .await;
            assert_eq!(
                original.is_ok(),
                matches!(
                    case,
                    "trusted" | "absent" | "stale" | "invalid" | "unsupported"
                )
            );
            let actual = assess(&store, &caller, &mut session, &policy)
                .unwrap()
                .await;
            if case == "unsupported" {
                assert!(original.is_ok());
                assert!(matches!(actual, Err(AdmittedError::Unsupported(_))));
            } else {
                compare(original, actual.unwrap(), &policy);
            }
            assert_eq!(root.available_bytes(), idle);
            assert_open_transaction(&store, &mut session, "assessment-marker").await;
            session.abort_transaction().await.unwrap();
            assert_eq!(
                store
                    .agents
                    .find_one(doc! {"_id":agent.id.as_str()})
                    .await
                    .unwrap()
                    .unwrap(),
                row
            );
            store
                .agent_log
                .delete_many(doc! {"agent_id":agent.id.as_str()})
                .await
                .unwrap();
        }
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn complete_assessment_keeps_full_scan_lineage_and_independent_summary_data() {
    let (client, name, mut store) = database().await;
    // This synthetic history also accepts duplicate sequences for corruption cases.
    store.agent_log = client
        .database(&name)
        .collection("assessment_corrupt_history");
    let mut agent = empty_agent(&store, "sbx_assessment_scan").await;
    agent.conversation.last_seq = 300;
    let expected = rows(&agent, 300, 16 * 1024);
    assert!(
        expected
            .iter()
            .map(|row| row.to_vec().unwrap().len())
            .sum::<usize>()
            > 4 * 1024 * 1024
    );
    store
        .agents
        .replace_one(
            doc! {"_id":agent.id.as_str()},
            agent_document(&agent).unwrap(),
        )
        .await
        .unwrap();
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
        ] {
            store
                .agent_log
                .delete_many(doc! {"agent_id":agent.id.as_str()})
                .await
                .unwrap();
            let mut documents = expected.clone();
            match case {
                "missing" => {
                    documents.remove(149);
                }
                "duplicate" => {
                    let mut duplicate = documents[149].clone();
                    duplicate.insert("_id", "assessment-duplicate");
                    documents.push(duplicate);
                }
                "extra" => {
                    documents.push(rows(&agent, 301, 8).pop().unwrap());
                }
                "malformed" => {
                    documents[149].insert("record", "invalid");
                }
                "workspace" | "worktree" | "late-created" => {
                    let mut entry = read_entry(&documents[0]).unwrap();
                    if case == "late-created" {
                        entry.seq = 150;
                    }
                    if let AgentLogKind::Created {
                        workspace_id,
                        worktree_id,
                    } = &mut entry.kind
                    {
                        if case == "workspace" {
                            workspace_id.0 = "ws_assessment_other".into();
                        }
                        if case == "worktree" {
                            worktree_id.0 = "wt_assessment_other".into();
                        }
                    }
                    documents[if case == "late-created" { 149 } else { 0 }] =
                        log_document(&entry).unwrap();
                }
                _ => {}
            }
            store.agent_log.insert_many(documents).await.unwrap();
            let mut session = client.start_session().await.unwrap();
            begin(&store, &mut session).await;
            let original =
                scan_complete_log_with_session(ScanCaller::Other, &store, &agent, &mut session)
                    .await;
            assert_eq!(original.is_ok(), case == "valid");
            if matches!(case, "workspace" | "worktree" | "late-created") {
                assert!(
                    matches!(&original, Err(TransactionLogError::Domain(SandboxStoreError::Corrupt(message))) if message == "Agent creation log lineage does not match")
                );
            }
            let actual = assess(&store, &agent, &mut session, &policy)
                .unwrap()
                .await
                .unwrap();
            compare(original, actual, &policy);
            assert_eq!(root.available_bytes(), idle);
            if case == "valid" {
                let first = assess(&store, &agent, &mut session, &policy)
                    .unwrap()
                    .await
                    .unwrap()
                    .transpose()
                    .unwrap();
                let first_held = idle - root.available_bytes();
                let second = assess(&store, &agent, &mut session, &policy)
                    .unwrap()
                    .await
                    .unwrap()
                    .transpose()
                    .unwrap();
                assert_eq!(*first, *second);
                assert_eq!(idle - root.available_bytes(), first_held * 2);
                assert_eq!(
                    policy
                        .budget(IngressClass::Transformation)
                        .available_bytes(),
                    policy.bounds().transformation_bytes()
                );
                drop(first);
                assert_eq!(idle - root.available_bytes(), first_held);
                drop(second);
                assert_eq!(root.available_bytes(), idle);
            }
            assert_open_transaction(&store, &mut session, "assessment-marker").await;
            session.abort_transaction().await.unwrap();
        }
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn complete_assessment_preserves_transaction_replacements_refusal_and_cancellation() {
    let (client, name, store) = database().await;
    let agent = empty_agent(&store, "sbx_assessment_transaction").await;
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = client.start_session().await.unwrap();
    begin(&store, &mut session).await;
    drop(assess(&store, &agent, &mut session, &policy).unwrap());
    assert_eq!(root.available_bytes(), idle);
    for class in [IngressClass::Data, IngressClass::Transformation] {
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
        match assess(&store, &agent, &mut session, &policy) {
            Ok(work) => assert!(matches!(work.await, Err(AdmittedError::Refused(_)))),
            Err(error) => assert!(matches!(error, AdmittedError::Refused(_))),
        }
        drop(pressure);
        assert_open_transaction(&store, &mut session, "assessment-marker").await;
        assert_eq!(root.available_bytes(), idle);
    }
    let mut changed = agent.clone();
    changed.revision += 1;
    changed.profile.content.system_prompt = "changed".repeat(16384);
    store
        .agents
        .replace_one(
            doc! {"_id":agent.id.as_str()},
            agent_document(&changed).unwrap(),
        )
        .session(&mut session)
        .await
        .unwrap();
    for caller in [&agent, &changed] {
        let original =
            scan_complete_log_with_session(ScanCaller::Other, &store, caller, &mut session).await;
        let actual = assess(&store, caller, &mut session, &policy)
            .unwrap()
            .await
            .unwrap();
        compare(original, actual, &policy);
        assert_eq!(root.available_bytes(), idle);
    }
    assert_open_transaction(&store, &mut session, "assessment-marker").await;
    session.abort_transaction().await.unwrap();
    let result = assess(&store, &agent, &mut session, &policy)
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    assert_eq!(*result, LogSummary::default());
    drop(result);
    assert_eq!(root.available_bytes(), idle);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn complete_assessment_retains_original_find_and_get_more_errors() {
    let (client, name, store) = database().await;
    let mut agent = empty_agent(&store, "sbx_assessment_driver").await;
    agent.conversation.last_seq = 200;
    store
        .agents
        .replace_one(
            doc! {"_id":agent.id.as_str()},
            agent_document(&agent).unwrap(),
        )
        .await
        .unwrap();
    store
        .agent_log
        .insert_many(rows(&agent, 200, 64))
        .await
        .unwrap();
    let mut options =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some("cma-assessment-error".into());
    let reader = mongodb::Client::with_options(options).unwrap();
    let mut store = MongoSandboxStore::open(&reader, &name).await.unwrap();
    let admin = client.database("admin");
    let (root, policy) = policy();
    let idle = root.available_bytes();
    for cooperation in [false, true] {
        store = store.with_scan_cooperation(cooperation);
        for command in ["find", "getMore"] {
            let mut session = reader.start_session().await.unwrap();
            start_transaction(&mut session).await.unwrap();
            admin.run_command(doc! {"configureFailPoint":"failCommand", "mode":{"times":1}, "data":{
                "failCommands":[command], "appName":"cma-assessment-error", "errorCode":112, "errorLabels":["TransientTransactionError"]
            }}).await.unwrap();
            let error = assess(&store, &agent, &mut session, &policy)
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
                panic!("driver failure required")
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
    drop(store);
    reader.shutdown().await;
    client.database(&name).drop().await.unwrap();
}
