use super::*;

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn closed_admission_claims_only_expiring_and_preserves_live_authority() {
    let fixture = Fixture::new().await;
    let mut value = expired();
    fixture
        .store
        .backups
        .insert_one(backup_document(&value).unwrap())
        .await
        .unwrap();
    let holder = format!("expiry-fixture:{}", uuid::Uuid::new_v4());
    let now = layer2_core::now_rfc3339();
    let until = at_offset(60);
    assert!(fixture
        .store
        .claim_expiring(&now, &holder, &until)
        .await
        .unwrap()
        .is_none());
    value.state = HomeBackupState::Expiring;
    fixture
        .store
        .backups
        .replace_one(doc! {"_id":&value.id}, backup_document(&value).unwrap())
        .await
        .unwrap();
    let claimed = fixture
        .store
        .claim_expiring(&now, &holder, &until)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.state, HomeBackupState::Expiring);
    assert!(fixture
        .store
        .claim_expiring(&now, &holder, &until)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        fixture.store.get_backup(&value.id).await.unwrap(),
        Some(claimed)
    );
    fixture.cleanup().await;
}

pub(super) fn at_offset(seconds: i64) -> String {
    let now = std::time::SystemTime::now();
    let duration = std::time::Duration::from_secs(seconds.unsigned_abs());
    layer2_core::rfc3339_at(if seconds >= 0 {
        now + duration
    } else {
        now - duration
    })
}

pub(super) fn expired() -> HomeBackup {
    let mut value = standalone_backup(HomeBackupState::Ready);
    value.cut.requested_at = at_offset(-7200);
    value.cut.source_creation_time = Some(at_offset(-7100));
    value.cut.ready_at = Some(at_offset(-7000));
    value.retention.expires_at = at_offset(-1);
    value
}

pub(super) fn proof(value: &HomeBackup) -> HomeBackupDeletionObservation {
    HomeBackupDeletionObservation {
        snapshot_name: value.storage.snapshot_name.clone(),
        source_reference: value.storage.source_reference.clone(),
        absent: true,
        observed_at: layer2_core::now_rfc3339(),
        receipt: "independent-absence-fixture".into(),
    }
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn expiry_refuses_pins_and_future_deadlines_then_claims_one_holder() {
    let fixture = Fixture::new().await;
    let mut value = expired();
    value.restore_pins = 1;
    fixture
        .store
        .backups
        .insert_one(backup_document(&value).unwrap())
        .await
        .unwrap();
    let holder = format!("expiry-fixture:{}", uuid::Uuid::new_v4());
    let now = layer2_core::now_rfc3339();
    let until = at_offset(60);
    assert!(fixture
        .store
        .claim_expiry(&now, &holder, &until)
        .await
        .unwrap()
        .is_none());
    value.restore_pins = 0;
    value.retention.expires_at = at_offset(3600);
    fixture
        .store
        .backups
        .replace_one(doc! {"_id":&value.id}, backup_document(&value).unwrap())
        .await
        .unwrap();
    assert!(fixture
        .store
        .claim_expiry(&now, &holder, &until)
        .await
        .unwrap()
        .is_none());
    value.retention.expires_at = at_offset(-1);
    fixture
        .store
        .backups
        .replace_one(doc! {"_id":&value.id}, backup_document(&value).unwrap())
        .await
        .unwrap();
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let other_client = Client::with_uri_str(&uri).await.unwrap();
    let other = MongoHomeBackupStore::open(&other_client, &fixture.database)
        .await
        .unwrap();
    let other_holder = super::holder();
    let (first, second) = tokio::join!(
        fixture.store.claim_expiry(&now, &holder, &until),
        other.claim_expiry(&now, &other_holder, &until)
    );
    let claimed: Vec<_> = [first, second]
        .into_iter()
        .filter_map(|result| match result {
            Ok(value) => value,
            Err(HomeBackupError::Conflict) => None,
            Err(error) => panic!("unexpected claim failure: {error}"),
        })
        .collect();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].state, HomeBackupState::Expiring);
    assert_eq!(
        fixture.store.get_backup(&value.id).await.unwrap(),
        Some(claimed[0].clone())
    );
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn deletion_requires_absence_and_rejects_stale_fences_before_publishing_receipt() {
    let fixture = Fixture::new().await;
    let value = expired();
    fixture
        .store
        .backups
        .insert_one(backup_document(&value).unwrap())
        .await
        .unwrap();
    let holder = format!("expiry-fixture:{}", uuid::Uuid::new_v4());
    let now = layer2_core::now_rfc3339();
    let claimed = fixture
        .store
        .claim_expiry(&now, &holder, &at_offset(60))
        .await
        .unwrap()
        .unwrap();
    let mut present = proof(&claimed);
    present.absent = false;
    assert_eq!(
        fixture.store.save_deletion(&claimed, &present, &now).await,
        Err(HomeBackupError::Conflict)
    );
    assert_eq!(
        fixture.store.get_backup(&value.id).await.unwrap(),
        Some(claimed.clone())
    );
    // Simulate expiry without waiting or changing the server's clock.
    let mut abandoned = claimed.clone();
    abandoned.lease.as_mut().unwrap().expires_at = at_offset(-1);
    fixture
        .store
        .backups
        .replace_one(doc! {"_id":&value.id}, backup_document(&abandoned).unwrap())
        .await
        .unwrap();
    let next = fixture
        .store
        .claim_expiry(&now, &holder, &at_offset(60))
        .await
        .unwrap()
        .unwrap();
    assert!(next.lease.as_ref().unwrap().fence > claimed.lease.as_ref().unwrap().fence);
    assert_eq!(
        fixture
            .store
            .save_deletion(&claimed, &proof(&claimed), &layer2_core::now_rfc3339())
            .await,
        Err(HomeBackupError::Conflict)
    );
    let completed = fixture
        .store
        .save_deletion(&next, &proof(&next), &layer2_core::now_rfc3339())
        .await
        .unwrap();
    assert_eq!(completed.state, HomeBackupState::Deleted);
    assert!(completed.erasure.is_some());
    assert!(completed.lease.is_none());
    assert_eq!(
        fixture.store.get_backup(&value.id).await.unwrap(),
        Some(completed)
    );
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn erasure_requires_a_fenced_quiescent_erasing_schedule_and_never_overrides_pins() {
    let fixture = Fixture::new().await;
    let mut value = expired();
    value.retention.expires_at = at_offset(86400);
    fixture
        .store
        .backups
        .insert_one(backup_document(&value).unwrap())
        .await
        .unwrap();
    let mut parent = schedule();
    parent.id = value.sandbox_id.clone();
    fixture.store.ensure_schedule(&parent).await.unwrap();
    let holder = format!("erasure-fixture:{}", uuid::Uuid::new_v4());
    let now = layer2_core::now_rfc3339();
    let until = at_offset(60);
    assert_eq!(
        fixture
            .store
            .claim_erasure(&value.id, &now, &holder, &until)
            .await,
        Err(HomeBackupError::Conflict)
    );
    parent.state = HomeBackupScheduleState::Erasing;
    parent.next_fence = 2;
    parent.revision = 2;
    fixture
        .store
        .schedules
        .replace_one(doc! {"_id":&parent.id}, schedule_document(&parent).unwrap())
        .await
        .unwrap();
    value.restore_pins = 1;
    fixture
        .store
        .backups
        .replace_one(doc! {"_id":&value.id}, backup_document(&value).unwrap())
        .await
        .unwrap();
    assert_eq!(
        fixture
            .store
            .claim_erasure(&value.id, &now, &holder, &until)
            .await,
        Err(HomeBackupError::Conflict)
    );
    assert_eq!(
        fixture.store.get_schedule(&parent.id).await.unwrap(),
        Some(parent.clone())
    );
    value.restore_pins = 0;
    fixture
        .store
        .backups
        .replace_one(doc! {"_id":&value.id}, backup_document(&value).unwrap())
        .await
        .unwrap();
    let claimed = fixture
        .store
        .claim_erasure(&value.id, &now, &holder, &until)
        .await
        .unwrap();
    assert_eq!(claimed.state, HomeBackupState::Expiring);
    assert_eq!(
        fixture
            .store
            .get_schedule(&parent.id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        3
    );
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn expiry_refuses_authority_expired_on_server_even_with_a_stale_client_clock() {
    let fixture = Fixture::new().await;
    let value = expired();
    fixture
        .store
        .backups
        .insert_one(backup_document(&value).unwrap())
        .await
        .unwrap();
    let holder = format!("expiry-fixture:{}", uuid::Uuid::new_v4());
    assert!(fixture
        .store
        .claim_expiry(&at_offset(-60), &holder, &at_offset(-1))
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        fixture.store.get_backup(&value.id).await.unwrap(),
        Some(value)
    );
    fixture.cleanup().await;
}
