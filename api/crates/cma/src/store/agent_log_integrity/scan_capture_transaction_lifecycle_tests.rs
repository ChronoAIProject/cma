use super::super::super::fixtures::owned;
use super::*;
use std::{
    future::{poll_fn, Future},
    pin::Pin,
    task::Poll,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn transaction_capture_observes_only_its_callers_uncommitted_agent_and_rows() {
    let (client, name, store) = database().await;
    let (agent, _) = seed(&store, 0).await;
    let mut caller = agent.clone();
    caller.conversation.last_seq = 2;
    caller.revision += 1;
    let rows = [
        AgentLogKind::Created {
            workspace_id: caller.workspace_id.clone(),
            worktree_id: caller.creation_worktree_id().clone(),
        },
        AgentLogKind::AgentMessage {
            content: "uncommitted input".into(),
        },
    ]
    .into_iter()
    .enumerate()
    .map(|(index, kind)| {
        log_document(&AgentLogEntry {
            agent_id: caller.id.clone(),
            seq: index as u64 + 1,
            appended_at: "2026-01-01T00:00:00Z".into(),
            kind,
        })
        .unwrap()
    })
    .collect::<Vec<_>>();
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    store
        .agents
        .replace_one(
            doc! {"_id":caller.id.as_str()},
            agent_document(&caller).unwrap(),
        )
        .session(&mut session)
        .await
        .unwrap();
    store
        .agent_log
        .insert_many(rows.clone())
        .session(&mut session)
        .await
        .unwrap();
    let captured = capture_complete_log_with_session_admitted(
        ScanCaller::Other,
        &store,
        &caller,
        &mut session,
        &policy,
        plan(),
    )
    .unwrap()
    .await
    .unwrap()
    .transpose()
    .unwrap();
    assert_eq!(captured.summary.entry_count(), 2);
    assert_eq!(captured.entries.as_slice().len(), 2);
    for (entry, expected) in captured.entries.as_slice().iter().zip(&rows) {
        assert_eq!(**entry, read_entry(expected).unwrap());
    }
    assert_eq!(
        store
            .agents
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        agent_document(&agent).unwrap()
    );
    assert_eq!(
        store
            .agent_log
            .count_documents(doc! {"agent_id":agent.id.as_str()})
            .await
            .unwrap(),
        0
    );
    drop(captured);
    session.abort_transaction().await.unwrap();
    assert_eq!(root.available_bytes(), idle);
    assert_eq!(
        store
            .agent_log
            .count_documents(doc! {"agent_id":agent.id.as_str()})
            .await
            .unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn transaction_capture_preserves_replacement_checkpoint_cancellation_and_pressure() {
    let (client, name, mut store) = database().await;
    let (agent, rows) = seed(&store, 300).await;
    store = store.with_scan_cooperation(true);
    store.scan_cooperation.section_budget = std::time::Duration::ZERO;
    let (root, policy) = policy();
    let idle = root.available_bytes();
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
        let mut session = client.start_session().await.unwrap();
        begin(&store, &mut session).await;
        let before = policy.budget(IngressClass::Data).available_bytes();
        let fresh = store
            .integrity_agent_read_admitted(&agent.id, &mut session, false, &policy, plan())
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
            let mut work = capture_complete_log_with_session_admitted(
                ScanCaller::Other,
                &store,
                &agent,
                &mut session,
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
                policy.bounds().data_bytes() - policy.budget(IngressClass::Data).available_bytes(),
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
        assert_open_transaction(&store, &mut session, "capture-report-marker").await;
        session.abort_transaction().await.unwrap();
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn transaction_capture_keeps_unpolled_refusal_and_original_driver_error_ownership() {
    let (client, name, store) = database().await;
    let (agent, _) = seed(&store, 200).await;
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = client.start_session().await.unwrap();
    drop(
        capture_complete_log_with_session_admitted(
            ScanCaller::Other,
            &store,
            &agent,
            &mut session,
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
        match capture_complete_log_with_session_admitted(
            ScanCaller::Other,
            &store,
            &agent,
            &mut session,
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
        let error = capture_complete_log_with_session_admitted(
            ScanCaller::Other,
            &store,
            &agent,
            &mut session,
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
