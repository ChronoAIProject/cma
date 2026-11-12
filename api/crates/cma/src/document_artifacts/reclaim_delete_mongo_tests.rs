use super::*;

pub(super) fn unclaimed(candidate: &ObjectCandidate) -> DeleteClaim {
    DeleteClaim {
        candidate: candidate.clone(),
        holder: uuid::Uuid::new_v4().to_string(),
        fence: 1,
        lease_until: 300_000,
    }
}
pub(super) async fn claimed(
    store: &MongoDocumentReclaimStore,
    candidate: &ObjectCandidate,
) -> DeleteClaim {
    store
        .claim_delete(candidate, &uuid::Uuid::new_v4().to_string(), 1_000)
        .await
        .unwrap()
        .unwrap()
}
async fn condemned() -> (
    Database,
    MongoDocumentQuotaStore,
    MongoDocumentReclaimStore,
    ObjectCandidate,
) {
    let (db, quota, store) = fixture().await;
    let reservation = source(&record().await, b"source");
    quota.reserve(&reservation).await.unwrap();
    age(&quota, &reservation).await;
    let candidate = candidate(&store).await;
    assert_eq!(store.judge(&candidate).await.unwrap(), Judgement::Condemned);
    (db, quota, store, candidate)
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_reclaim_delete_claim_is_exclusive_across_two_stores() {
    let (db, quota, first, candidate) = condemned().await;
    let second = MongoDocumentReclaimStore::new(&db, true);
    let holder_a = uuid::Uuid::new_v4().to_string();
    let holder_b = uuid::Uuid::new_v4().to_string();
    let (a, b) = tokio::join!(
        first.claim_delete(&candidate, &holder_a, 1_000),
        second.claim_delete(&candidate, &holder_b, 1_000)
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(usize::from(a.is_some()) + usize::from(b.is_some()), 1);
    let claim = a.or(b).unwrap();
    assert_eq!(claim.fence, 1);
    assert_eq!(claim.lease_until, 301_000);
    assert!(first
        .claim_delete(&candidate, &claim.holder, 1_001)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        owner(&quota, &candidate.source)
            .await
            .get_i64("objects")
            .unwrap(),
        1
    );
    assert!(first.finalize(&claim).await.unwrap());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_reclaim_stale_claimant_cannot_finalize_after_takeover() {
    let (db, quota, first, candidate) = condemned().await;
    let second = MongoDocumentReclaimStore::new(&db, true);
    let old = claimed(&first, &candidate).await;
    let new_holder = uuid::Uuid::new_v4().to_string();
    assert!(second
        .claim_delete(&candidate, &new_holder, old.lease_until)
        .await
        .unwrap()
        .is_none());
    let new = second
        .claim_delete(&candidate, &new_holder, old.lease_until + 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(new.fence, old.fence + 1);
    assert!(!first.finalize(&old).await.unwrap());
    let mut wrong_holder = new.clone();
    wrong_holder.holder = old.holder.clone();
    assert!(!first.finalize(&wrong_holder).await.unwrap());
    let mut wrong_fence = new.clone();
    wrong_fence.fence = old.fence;
    assert!(!first.finalize(&wrong_fence).await.unwrap());
    let usage = owner(&quota, &candidate.source).await;
    assert_eq!(usage.get_i64("objects").unwrap(), 1);
    assert_eq!(usage.get_i64("bytes").unwrap(), 6);
    assert!(second.finalize(&new).await.unwrap());
    assert!(!first.finalize(&old).await.unwrap());
    assert!(!second.finalize(&new).await.unwrap());
    let usage = owner(&quota, &candidate.source).await;
    for key in ["documents", "bytes", "objects"] {
        assert_eq!(usage.get_i64(key).unwrap(), 0);
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_reclaim_unclaimed_row_cannot_be_finalized() {
    let (db, quota, store, candidate) = condemned().await;
    assert!(!store.finalize(&unclaimed(&candidate)).await.unwrap());
    assert_eq!(
        owner(&quota, &candidate.source)
            .await
            .get_i64("objects")
            .unwrap(),
        1
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_reclaim_stale_candidate_never_releases_a_new_reservation_incarnation() {
    for legacy in [false, true] {
        let (db, quota, reclaim) = fixture().await;
        let reservation = source(&record().await, b"source");
        quota.reserve(&reservation).await.unwrap();
        if legacy {
            quota
                .rows
                .update_one(
                    doc! {"_id":object_id(&reservation)},
                    doc! {"$unset":{"reservation_id":""}},
                )
                .await
                .unwrap();
        }
        age(&quota, &reservation).await;
        let old = candidate(&reclaim).await;
        reclaim.judge(&old).await.unwrap();
        let old_claim = claimed(&reclaim, &old).await;
        assert!(reclaim.finalize(&old_claim).await.unwrap());
        quota.reserve(&reservation).await.unwrap();
        age(&quota, &reservation).await;
        let new = candidate(&reclaim).await;
        assert_ne!(old.reservation_id, new.reservation_id);
        assert_eq!(reclaim.judge(&old).await.unwrap(), Judgement::Skipped);
        assert_eq!(reclaim.judge(&new).await.unwrap(), Judgement::Condemned);
        assert!(reclaim
            .claim_delete(&old, &old_claim.holder, 1_000)
            .await
            .unwrap()
            .is_none());
        let new_claim = reclaim
            .claim_delete(&new, &old_claim.holder, 1_000)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(old_claim.fence, new_claim.fence);
        assert!(!reclaim.finalize(&old_claim).await.unwrap());
        let usage = owner(&quota, &reservation).await;
        assert_eq!(usage.get_i64("objects").unwrap(), 1);
        assert_eq!(usage.get_i64("bytes").unwrap(), 6);
        assert!(reclaim.finalize(&new_claim).await.unwrap());
        db.drop().await.unwrap();
    }
}
