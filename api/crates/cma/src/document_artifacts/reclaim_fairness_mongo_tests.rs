use super::*;

struct FailingDeletion;
#[async_trait::async_trait]
impl DocumentBlobMaintenance for FailingDeletion {
    async fn delete(&self, _: &str) -> Result<BlobDeletion, BlobMaintenanceError> {
        Err(DocumentError::Unavailable.into())
    }
    async fn list(&self, _: &str, _: Option<&str>) -> Result<BlobPage, BlobMaintenanceError> {
        unreachable!()
    }
    fn deletion_supported(&self) -> bool {
        true
    }
}

async fn noisy_rows(quota: &MongoDocumentQuotaStore, lease_until: Option<i64>) {
    let metadata = record().await;
    let rows: Vec<_> = (0..RECLAIM_BATCH)
        .map(|index| {
            let reservation = source(&metadata, index.to_string().as_bytes());
            let mut row = object_row(&reservation).unwrap();
            row.insert("state", "condemned");
            row.insert("reserved_at", DateTime::from_millis(1));
            if let Some(until) = lease_until {
                row.insert("delete_holder", uuid::Uuid::new_v4().to_string());
                row.insert("delete_fence", 1_i64);
                row.insert("delete_lease_until", DateTime::from_millis(until));
            }
            row
        })
        .collect();
    quota.rows.insert_many(rows).await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_reclaim_failing_condemned_owner_cannot_starve_reserved_discovery() {
    let (db, quota, reclaim) = fixture().await;
    noisy_rows(&quota, None).await;
    let mut other = source(&record().await, b"other-owner-orphan");
    other.owner_hash = digest(b"other-reclaim-owner");
    quota.reserve(&other).await.unwrap();
    age(&quota, &other).await;
    let maintenance = DocumentMaintenance::new(Arc::new(reclaim), Arc::new(FailingDeletion), true);
    let mut judged = 0;
    for _ in 0..3 {
        judged += maintenance.reclaim_step(3_600_000).await.unwrap().judged;
    }
    let row = quota
        .rows
        .find_one(doc! {"_id": object_id(&other)})
        .await
        .unwrap()
        .unwrap();
    db.drop().await.unwrap();
    assert_eq!(judged, 1);
    assert_eq!(row.get_str("state").unwrap(), "condemned");
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_reclaim_active_leases_do_not_fill_candidate_batch_and_expiry_recovers() {
    let (db, quota, reclaim) = fixture().await;
    let before = 10_000;
    let now = before + RECLAIM_GRACE.as_millis() as i64;
    noisy_rows(&quota, Some(now)).await;
    let mut other = source(&record().await, b"other-owner-orphan");
    other.owner_hash = digest(b"other-reclaim-owner");
    quota.reserve(&other).await.unwrap();
    age(&quota, &other).await;
    let candidates = reclaim.candidates(before, RECLAIM_BATCH).await.unwrap();
    assert_eq!(
        candidates.len(),
        1,
        "leases are active at their exact expiry timestamp"
    );
    assert_eq!(candidates[0].source, other);
    let expired = reclaim.candidates(before + 1, RECLAIM_BATCH).await.unwrap();
    assert_eq!(
        expired.iter().filter(|row| row.condemned).count(),
        RECLAIM_BATCH
    );
    assert_eq!(expired.iter().filter(|row| !row.condemned).count(), 1);
    let old = &expired[0];
    let claim = reclaim
        .claim_delete(old, &uuid::Uuid::new_v4().to_string(), now + 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.fence, 2);
    assert!(reclaim
        .claim_delete(old, &uuid::Uuid::new_v4().to_string(), now + 2)
        .await
        .unwrap()
        .is_none());
    db.drop().await.unwrap();
}
