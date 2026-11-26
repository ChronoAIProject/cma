use super::worker_fixture::*;
use crate::store::account_deletions::commit_pause::{Pause, Point};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn absence_requires_a_committed_parent_fence_before_retrying_the_same_operation() {
    let (db, _, journal, claim) = claimed().await;
    let request = plan(&claim, "uncertain");
    let before = snapshot(&db).await;
    assert!(matches!(
        journal.resolve(&request, None, &budget()).await.unwrap(),
        JournalResolution::Unresolved
    ));
    assert_eq!(snapshot(&db).await, before);
    let JournalResolution::NotCommittedAfterFence {
        operation,
        claim: next,
    } = journal
        .resolve(&request, Some(&claim), &budget())
        .await
        .unwrap()
    else {
        panic!("a successful parent fence must settle absence")
    };
    assert_eq!(operation, request.key());
    assert_eq!(
        next.checkpoint().parent_revision,
        claim.checkpoint().parent_revision + 1
    );
    assert!(matches!(
        journal.commit(&claim, &request, &budget()).await,
        Err(JournalError::StaleClaim)
    ));
    let (receipt, _) = recorded(journal.commit(&next, &request, &budget()).await.unwrap());
    assert_eq!(receipt.operation, operation);
    let second = reopened(&db, None).await;
    let JournalResolution::Committed(resolved) =
        second.resolve(&request, None, &budget()).await.unwrap()
    else {
        panic!("receipt must resolve")
    };
    assert_eq!(receipt, resolved);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn an_inflight_parent_write_keeps_missing_receipt_unresolved() {
    let (db, _, mut journal, claim) = claimed().await;
    let request = plan(&claim, "pending");
    let second = reopened(&db, None).await;
    let pause = Pause::new(Point::AfterGuard);
    journal.pause = Some(pause.clone());
    let req = request.clone();
    let old = claim.clone();
    let write = tokio::spawn(async move { journal.commit(&old, &req, &budget()).await });
    pause.reached().await;
    assert!(matches!(
        second.resolve(&request, None, &budget()).await.unwrap(),
        JournalResolution::Unresolved
    ));
    assert!(matches!(
        second
            .resolve(&request, Some(&claim), &budget())
            .await
            .unwrap(),
        JournalResolution::Unresolved
    ));
    pause.release();
    let (receipt, _) = recorded(write.await.unwrap().unwrap());
    let JournalResolution::Committed(recovered) =
        second.resolve(&request, None, &budget()).await.unwrap()
    else {
        panic!("committed receipt required")
    };
    assert_eq!(receipt, recovered);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn lost_commit_acknowledgements_resolve_the_original_receipt_after_restart() {
    let (db, _, _, claim) = claimed().await;
    let app = format!("erasure-journal-{}", uuid::Uuid::new_v4().simple());
    let journal = reopened(&db, Some(&app)).await;
    let admin =
        mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    admin.database("admin").run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":5},"data":{"failCommands":["commitTransaction"],"appName":&app,"writeConcernError":{"code":50,"codeName":"MaxTimeMSExpired","errmsg":"synthetic journal ambiguity"}}}).await.unwrap();
    let request = plan(&claim, "lost-reply");
    let result = journal.commit(&claim, &request, &budget()).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(
        matches!(result.unwrap(),JournalOutcome::RecoveryPending {operation} if operation == request.key())
    );
    let second = reopened(&db, None).await;
    let JournalResolution::Committed(receipt) =
        second.resolve(&request, None, &budget()).await.unwrap()
    else {
        panic!("original committed receipt required")
    };
    let before = snapshot(&db).await;
    let (replayed, authority) = recorded(second.commit(&claim, &request, &budget()).await.unwrap());
    assert_eq!(receipt, replayed);
    assert!(authority.is_none());
    assert_eq!(snapshot(&db).await, before);
    assert_eq!(
        journal
            .operations
            .count_documents(doc! {"_id":request.key()})
            .await
            .unwrap(),
        1
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn timeout_after_commit_submission_preserves_uncertainty_until_the_receipt_arrives() {
    let (db, _, _, claim) = claimed().await;
    let app = format!("erasure-timeout-{}", uuid::Uuid::new_v4().simple());
    let journal = reopened(&db, Some(&app)).await;
    let admin =
        mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    admin.database("admin").run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},"data":{"failCommands":["commitTransaction"],"appName":&app,"blockConnection":true,"blockTimeMS":3000}}).await.unwrap();
    let request = plan(&claim, "delayed-commit");
    let short = JournalBudget::new(Duration::from_secs(1)).unwrap();
    let result = journal.commit(&claim, &request, &short).await;
    assert!(
        matches!(result.unwrap(),JournalOutcome::RecoveryPending {operation} if operation == request.key())
    );
    let second = reopened(&db, None).await;
    assert!(matches!(
        second.resolve(&request, None, &budget()).await.unwrap(),
        JournalResolution::Unresolved
    ));
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    let receipt = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let JournalResolution::Committed(receipt) =
                second.resolve(&request, None, &budget()).await.unwrap()
            {
                break receipt;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(receipt.operation, request.key());
    assert_eq!(
        journal
            .operations
            .count_documents(doc! {"_id":request.key()})
            .await
            .unwrap(),
        1
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn cancellation_before_commit_aborts_all_staged_journal_state() {
    let (db, _, mut journal, claim) = claimed().await;
    let request = plan(&claim, "cancelled");
    let before = snapshot(&db).await;
    let pause = Pause::new(Point::AfterGuard);
    journal.pause = Some(pause.clone());
    let budget = budget();
    let copied = budget.clone();
    let write = tokio::spawn(async move { journal.commit(&claim, &request, &copied).await });
    pause.reached().await;
    budget.cancel();
    pause.release();
    assert!(matches!(write.await.unwrap(), Err(JournalError::Cancelled)));
    assert_eq!(snapshot(&db).await, before);
    db.drop().await.unwrap();
}
