use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn selected_retry_retries_a_transaction_conflict_without_double_counting_work() {
    let f = Fixture::new().await;
    let (current, now) = f.marked_claim().await;
    let admin = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
        .await
        .unwrap();
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": {"times": 1_i32},
            "data": {"failCommands": ["update"], "appName": &f.app,
                "errorCode": 112_i32, "errorLabels": ["TransientTransactionError"]}
        })
        .await
        .unwrap();
    f.reset_scans();
    let outcome = f.fail(&current, now).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    outcome.unwrap();
    assert_eq!(
        f.scans(),
        2,
        "each retried transaction validates its own complete snapshot"
    );
    assert_retry(&current, &f.h.stored(&current.id).await, 30);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn selected_retry_refuses_unknown_commit_without_normal_append_or_new_settlement() {
    let f = Fixture::new().await;
    let (current, now) = f.marked_claim().await;
    let admin = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
        .await
        .unwrap();
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": "alwaysOn",
            "data": {"failCommands": ["commitTransaction"], "appName": &f.app,
                "errorCode": 1_i32, "errorLabels": ["UnknownTransactionCommitResult"]}
        })
        .await
        .unwrap();
    f.reset_scans();
    let outcome = f.fail(&current, now).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    assert!(
        matches!(outcome, Err(SandboxStoreError::Unavailable(_))),
        "{outcome:?}"
    );
    assert_eq!(
        f.scans(),
        1,
        "an unknown commit cannot authorize another publication"
    );
    assert_eq!(f.h.stored(&current.id).await, current);
    assert!(!f
        .h
        .worker
        .run_one(&current.id, now + Duration::from_secs(1))
        .await
        .unwrap());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn selected_retry_proves_a_committed_unknown_result_without_republishing() {
    let f = Fixture::new().await;
    let (current, now) = f.marked_claim().await;
    let admin = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
        .await
        .unwrap();
    admin.database("admin").run_command(doc! {
        "configureFailPoint": "failCommand", "mode": {"times": 5_i32},
        "data": {"failCommands": ["commitTransaction"], "appName": &f.app,
            "writeConcernError": {"code": 50_i32, "codeName": "MaxTimeMSExpired", "errmsg": "fixture unknown commit"}}
    }).await.unwrap();
    f.reset_scans();
    let outcome = f.fail(&current, now).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    outcome.unwrap();
    assert_eq!(
        f.scans(),
        2,
        "publication and exact commit proof each validate once"
    );
    assert_retry(&current, &f.h.stored(&current.id).await, 30);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn concurrent_selected_settlements_have_one_revision_winner() {
    let f = Fixture::new().await;
    let (current, now) = f.marked_claim().await;
    let proposal = f.selected(&current, now).await;
    // Independent store instances model separate API replicas; they do not
    // share the per-process Agent writer mutex.
    let peer = MongoSandboxStore::open(&f.client, &f.name).await.unwrap();
    f.reset_scans();
    let (left, right) = tokio::join!(
        f.h.store.commit_agent_work(proposal.clone()),
        peer.commit_agent_work(proposal),
    );
    assert!(matches!(
        (left, right),
        (Ok(()), Err(SandboxStoreError::Conflict)) | (Err(SandboxStoreError::Conflict), Ok(()))
    ));
    assert!(f.scans() >= 2);
    assert_retry(&current, &f.h.stored(&current.id).await, 30);
    f.cleanup().await;
}
