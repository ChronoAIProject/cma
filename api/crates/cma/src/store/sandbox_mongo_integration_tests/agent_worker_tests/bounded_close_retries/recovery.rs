use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn close_retry_restart_mixed_writers_and_kill_switch_preserve_the_claim() {
    let mut f = Fixture::new(500).await;
    f.store = MongoSandboxStore::open(&f.client, &f.name)
        .await
        .unwrap()
        .with_fleet_log_percentage(100)
        .with_bounded_close_retries(true);
    scans::isolated_test(async {
        let mutation = retry(&f.agent);
        let (left, right) = tokio::join!(
            f.store.commit_agent_work(mutation.clone()),
            f.store.commit_agent_work(mutation)
        );
        assert_eq!(
            [left.is_ok(), right.is_ok()].iter().filter(|v| **v).count(),
            1
        );
        assert_eq!(scans::snapshot(CALLER, true)[0], 0);
    })
    .await;
    let current = claim(&f.store, &f.agent.id, TWO_MINUTES).await;
    let mut mutation = retry(&current);
    mutation.appended_at = TWO_MINUTES.into();
    mutation.agent.updated_at = TWO_MINUTES.into();
    mutation.agent.work.ready_at = "2026-09-05T00:03:00Z".into();
    // Compatible full-path writers can interleave and maintain the same proof.
    let old = MongoSandboxStore::open(&f.client, &f.name).await.unwrap();
    scans::isolated_test(async {
        old.commit_agent_work(mutation.clone()).await.unwrap();
        assert_eq!(scans::snapshot(CALLER, true)[0], 1);
    })
    .await;
    assert_eq!(
        f.store.get_agent(&f.agent.id).await.unwrap(),
        Some(mutation.agent)
    );
    // The claim changes revision without changing history; the proof survives.
    let current = claim(&f.store, &f.agent.id, "2026-09-05T00:03:00Z").await;
    let mut mutation = retry(&current);
    mutation.appended_at = "2026-09-05T00:03:00Z".into();
    mutation.agent.updated_at = mutation.appended_at.clone();
    mutation.agent.work.ready_at = "2026-09-05T00:04:00Z".into();
    scans::isolated_test(async {
        f.store.commit_agent_work(mutation).await.unwrap();
        assert_eq!(scans::snapshot(CALLER, true)[0], 0);
    })
    .await;
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn close_retry_transient_commit_does_not_duplicate_failure_events() {
    let f = Fixture::new(500).await;
    let admin = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
        .await
        .unwrap();
    admin.database("admin").run_command(doc! { "configureFailPoint": "failCommand", "mode": { "times": 1 },
        "data": { "failCommands": ["update"], "appName": &f.app, "errorCode": 112, "errorLabels": ["TransientTransactionError"] }
    }).await.unwrap();
    let result = f.store.commit_agent_work(retry(&f.agent)).await;
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .unwrap();
    result.unwrap();
    assert_eq!(
        f.store
            .agent_log
            .count_documents(doc! { "agent_id": f.agent.id.as_str() })
            .await
            .unwrap(),
        501
    );
    assert_eq!(
        f.store.get_agent(&f.agent.id).await.unwrap(),
        Some(retry(&f.agent).agent)
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn close_retry_unknown_commit_uses_complete_receipt_proof_without_reappend() {
    let f = Fixture::new(500).await;
    let admin = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
        .await
        .unwrap();
    admin.database("admin").run_command(doc! { "configureFailPoint": "failCommand", "mode": { "times": 5 },
        "data": { "failCommands": ["commitTransaction"], "appName": &f.app,
            "writeConcernError": { "code": 50, "codeName": "MaxTimeMSExpired", "errmsg": "injected ambiguous commit" } }
    }).await.unwrap();
    let (result, unknown_scans) = scans::isolated_test(async {
        let result = f.store.commit_agent_work(retry(&f.agent)).await;
        (
            result,
            scans::snapshot(ScanCaller::UnknownCommitProof, true)[0],
        )
    })
    .await;
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .unwrap();
    result.unwrap();
    assert_eq!(unknown_scans, 1);
    assert_eq!(
        f.store
            .agent_log
            .count_documents(doc! { "agent_id": f.agent.id.as_str() })
            .await
            .unwrap(),
        501
    );
    let mutation = retry(&f.agent);
    let receipt = PublicationReceipt {
        agent: mutation.agent,
        worktree: None,
        event: None,
        sign_in_sandbox: None,
        logs: vec![AgentLogEntry {
            agent_id: f.agent.id.clone(),
            seq: 501,
            appended_at: MINUTE.into(),
            kind: mutation.agent_logs[0].clone(),
        }],
    };
    assert!(reconcile_publication(&f.store, &receipt).await.unwrap());
    f.store
        .agent_log
        .delete_one(doc! { "agent_id": f.agent.id.as_str(), "seq": 250_i64 })
        .await
        .unwrap();
    assert_eq!(
        reconcile_publication(&f.store, &receipt).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert!(f.row().await.contains_key(QUARANTINE_FIELD));
    f.cleanup().await;
}
