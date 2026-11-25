use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retirement_between_snapshot_and_workspace_write_conflicts_and_retry_refuses() {
    let f = Fixture::new().await;
    f.arm(Pause::WorkspaceWrite);
    let store = f.store.clone();
    let request = publication(&f.original);
    let pending = tokio::spawn(async move { store.commit(request).await });
    f.wait().await;
    retire(&f.peer, &f.parent.id, true).await;
    let before = f.rows().await;
    f.release();
    assert_eq!(pending.await.unwrap(), Err(SandboxStoreError::Conflict));
    assert!(
        f.conflicts() >= 1,
        "existing parent event-head write must conflict with retirement"
    );
    assert_eq!(
        f.bodies(),
        2,
        "known abort must reread in a new transaction"
    );
    assert_eq!(f.rows().await, before);
    f.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn known_aborted_publication_rereads_parent_before_retry() {
    let f = Fixture::failpoints().await;
    f.fail(1, doc! {"failCommands":["update"], "errorCode":112_i32, "errorLabels":["TransientTransactionError"]}).await;
    f.arm(Pause::RetryRead);
    let store = f.store.clone();
    let request = publication(&f.original);
    let pending = tokio::spawn(async move { store.commit(request).await });
    f.wait().await;
    retire(&f.peer, &f.parent.id, false).await;
    let before = f.rows().await;
    f.release();
    let result = pending.await.unwrap();
    f.clear().await;
    assert_eq!(result, Err(SandboxStoreError::Conflict));
    assert_eq!(f.bodies(), 2);
    assert_eq!(f.rows().await, before);
    f.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn committed_ready_with_unknown_reply_reconciles_after_retirement_without_republication() {
    let f = Fixture::failpoints().await;
    f.fail(5, doc! {"failCommands":["commitTransaction"], "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"synthetic unknown workspace commit"}}).await;
    f.arm(Pause::CommitReply);
    let wake = f.store.agent_parent_ready.notified();
    tokio::pin!(wake);
    wake.as_mut().enable();
    let store = f.store.clone();
    let request = publication(&f.original);
    let pending = tokio::spawn(async move { store.commit(request).await });
    f.wait().await;
    // The injected write-concern error skips the server's majority wait.
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.peer.get_workspace(&f.original.id).await.unwrap() != Some(ready(&f.original)) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the committed Ready publication must become majority visible");
    retire(&f.peer, &f.parent.id, true).await;
    let before = f.rows().await;
    f.release();
    let result = pending.await.unwrap();
    f.clear().await;
    result.unwrap();
    tokio::time::timeout(Duration::from_secs(1), wake)
        .await
        .unwrap();
    assert_eq!(f.bodies(), 1);
    assert_eq!(f.rows().await, before);
    let commands = f.commands();
    let commits: Vec<_> = commands
        .iter()
        .filter(|(name, _)| name == "commitTransaction")
        .map(|(_, doc)| doc)
        .collect();
    assert_eq!(commits.len(), 5);
    for command in &commits {
        assert_eq!(command.get("lsid"), commits[0].get("lsid"));
        assert_eq!(command.get("txnNumber"), commits[0].get("txnNumber"));
    }
    let reads: Vec<_> = commands
        .iter()
        .skip_while(|(name, _)| name != "commitTransaction")
        .filter(|(name, _)| name == "find")
        .map(|(_, doc)| doc)
        .collect();
    assert_eq!(reads.len(), 2);
    let concern = reads[0].get_document("readConcern").unwrap();
    assert_eq!(concern.get_str("level").unwrap(), "snapshot");
    for command in &reads {
        assert_eq!(command.get_document("readConcern").unwrap(), concern);
    }
    f.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn uncommitted_unknown_result_cannot_be_replayed_after_parent_retirement() {
    let f = Fixture::failpoints().await;
    f.fail(5, doc! {"failCommands":["commitTransaction"],"errorCode":1_i32,"errorLabels":["UnknownTransactionCommitResult"]}).await;
    f.arm(Pause::ReconcileRead);
    let store = f.store.clone();
    let request = publication(&f.original);
    let pending = tokio::spawn(async move { store.commit(request).await });
    f.wait().await;
    let commands = f.commands();
    let session = commands
        .iter()
        .find(|(name, _)| name == "commitTransaction")
        .unwrap()
        .1
        .get_document("lsid")
        .unwrap()
        .clone();
    f.admin
        .database("admin")
        .run_command(doc! {"killSessions":[session]})
        .await
        .unwrap();
    f.clear().await;
    retire(&f.peer, &f.parent.id, true).await;
    let before = f.rows().await;
    f.release();
    assert!(matches!(
        pending.await.unwrap(),
        Err(SandboxStoreError::Unavailable(_))
    ));
    assert_eq!(f.bodies(), 1);
    assert_eq!(f.rows().await, before);
    f.close().await;
}
