use crate::store::account_deletions::commit_pause::{Pause, Point};
use crate::store::account_deletions::tests::worker_fixture::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn journal_expiry_at_final_guard_rolls_back_plan_renewal_receipt_and_checkpoint() {
    for renew in [false, true] {
        let (db, _, mut journal, checkpoint) = enrolled().await;
        let claim = acquire(&journal, checkpoint, 4).await;
        let request = if renew {
            JournalRequest::new(
                claim.checkpoint().binding.clone(),
                JournalCommand::Renew { lease_seconds: 300 },
            )
            .unwrap()
        } else {
            plan(&claim, "paused")
        };
        let before = snapshot(&db).await;
        let pause = Pause::new(Point::BeforeGuard);
        journal.pause = Some(pause.clone());
        let old = claim.clone();
        let write = tokio::spawn(async move { journal.commit(&old, &request, &budget()).await });
        pause.reached().await;
        wait_until(&db, &claim.lease().expires_at).await;
        pause.release();
        assert!(matches!(
            write.await.unwrap(),
            Err(JournalError::StaleClaim)
        ));
        assert_eq!(snapshot(&db).await, before);
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn journal_admitted_before_expiry_serializes_successor_claim_after_late_commit() {
    let (db, _, mut journal, checkpoint) = enrolled().await;
    let claim = acquire(&journal, checkpoint, 4).await;
    let request = plan(&claim, "serialized");
    let pause = Pause::new(Point::AfterGuard);
    journal.pause = Some(pause.clone());
    let second = reopened(&db, None).await;
    let old = claim.clone();
    let write = tokio::spawn(async move { journal.commit(&old, &request, &budget()).await });
    pause.reached().await;
    wait_until(&db, &claim.lease().expires_at).await;
    let takeover = JournalRequest::new(
        claim.checkpoint().binding.clone(),
        JournalCommand::Claim {
            request_id: "takeover".into(),
            holder: "successor".into(),
            lease_seconds: 90,
        },
    )
    .unwrap();
    assert!(matches!(
        second.claim_due(&takeover, &budget()).await,
        Err(JournalError::Conflict)
    ));
    pause.release();
    let (receipt, _) = recorded(write.await.unwrap().unwrap());
    let successor = acquire(&second, receipt.checkpoint, 90).await;
    assert!(successor.lease().fence > claim.lease().fence);
    assert_ne!(successor.lease().token, claim.lease().token);
    let before = snapshot(&db).await;
    assert!(second
        .commit(&claim, &plan(&claim, "stale"), &budget())
        .await
        .is_err());
    assert_eq!(snapshot(&db).await, before);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn malformed_or_expired_original_lease_cannot_be_renewed() {
    let (db, store, journal, claim) = claimed().await;
    let id = &claim.checkpoint().binding.deletion_id;
    for expiry in ["malformed", NOW] {
        db.collection::<Document>(RECORDS)
            .update_one(
                doc! {"_id":id.as_str()},
                doc! {"$set":{"work.lease.expires_at":expiry}},
            )
            .await
            .unwrap();
        let before = snapshot(&db).await;
        let current = store.get(id).await.unwrap().unwrap();
        let supplied = JournalClaim::from_record(claim.checkpoint().clone(), &current).unwrap();
        let request = JournalRequest::new(
            supplied.checkpoint().binding.clone(),
            JournalCommand::Renew { lease_seconds: 300 },
        )
        .unwrap();
        assert!(matches!(
            journal.commit(&supplied, &request, &budget()).await,
            Err(JournalError::Incompatible | JournalError::StaleClaim)
        ));
        assert_eq!(snapshot(&db).await, before);
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn cancellation_after_submission_returns_only_recovery_identity() {
    let (db, _, mut journal, claim) = claimed().await;
    let request = plan(&claim, "cancel-after-commit");
    let retained = request.clone();
    let second = reopened(&db, None).await;
    let pause = Pause::new(Point::AfterCommit);
    journal.pause = Some(pause.clone());
    let budget = budget();
    let copied = budget.clone();
    let write = tokio::spawn(async move { journal.commit(&claim, &request, &copied).await });
    pause.reached().await;
    budget.cancel();
    pause.release();
    assert!(
        matches!(write.await.unwrap().unwrap(),JournalOutcome::RecoveryPending {operation} if operation==retained.key())
    );
    assert!(matches!(
        second
            .resolve(&retained, None, &super::super::worker_fixture::budget())
            .await
            .unwrap(),
        JournalResolution::Committed(_)
    ));
    db.drop().await.unwrap();
}
