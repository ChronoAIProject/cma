use super::*;

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn same_sandbox_other_home_or_source_never_changes_current_class_or_previous_deadlines() {
    let fixture = Fixture::new().await;
    let mut previous = Vec::new();
    for (epoch, change) in [(91, "home"), (92, "source"), (93, "both")] {
        let mut value = standalone_backup(HomeBackupState::Ready);
        value.epoch = epoch;
        value.id = format!("hb-{}-{epoch}", value.sandbox_id);
        value.cut.ready_at = Some("2026-11-16T15:00:00Z".into());
        if change != "source" {
            value.source.home_identity = "other-home-fixture".into();
        }
        if change != "home" {
            value.source.home_source_id = "other-source-fixture".into();
        }
        fixture
            .store
            .backups
            .insert_one(backup_document(&value).unwrap())
            .await
            .unwrap();
        previous.push(value);
    }
    let claim = creating(&fixture.store).await;
    let (schedule, value) = ready(&claim, "2026-11-16T15:45:00Z");
    let done = fixture
        .store
        .save_capture(&claim, &schedule, &value, &layer2_core::now_rfc3339())
        .await
        .unwrap();
    let stored = done.backup.unwrap();
    assert_eq!(stored.retention.class, HomeBackupRetentionClass::Daily);
    assert_eq!(stored.retention.expires_at, "2026-12-16T15:45:00Z");
    for row in previous {
        assert_eq!(fixture.store.get_backup(&row.id).await.unwrap(), Some(row));
    }
    fixture.cleanup().await;
}

async fn creating(store: &MongoHomeBackupStore) -> HomeBackupClaim {
    store.ensure_schedule(&schedule()).await.unwrap();
    let now = layer2_core::now_rfc3339();
    let claim = store
        .claim_due(&now, &holder(), &until())
        .await
        .unwrap()
        .unwrap();
    // The domain clock is fixed; only Mongo lease authority uses wall time.
    let mut intent = backup(&claim);
    intent.cut.requested_at = "2026-11-16T00:00:00Z".into();
    intent.retention.expires_at = "2026-12-16T00:00:00Z".into();
    intent.validate().unwrap();
    let planned = store.plan_capture(&claim, &intent, &now).await.unwrap();
    let mut progress = planned.schedule.clone();
    progress.in_flight.as_mut().unwrap().phase = HomeBackupPhase::Creating;
    let mut value = planned.backup.clone().unwrap();
    value.state = HomeBackupState::Creating;
    store
        .save_capture(&planned, &progress, &value, &now)
        .await
        .unwrap()
}

fn ready(claim: &HomeBackupClaim, at: &str) -> (HomeBackupSchedule, HomeBackup) {
    let mut value = claim.backup.clone().unwrap();
    value.state = HomeBackupState::Ready;
    value.cut.source_creation_time = Some(value.cut.requested_at.clone());
    value.cut.ready_at = Some(at.into());
    value.storage.source_reference = Some("ready-copy-fixture".into());
    let ready_seconds = layer2_core::seconds_from_rfc3339(at).unwrap();
    value.retention.expires_at = layer2_core::rfc3339_at(
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(ready_seconds + 86400),
    );
    value.validate().unwrap();
    let mut completed = claim.schedule.clone();
    completed.in_flight = None;
    completed.last_ready_at = value.cut.ready_at.clone();
    completed.last_ready_backup_id = Some(value.id.clone());
    completed.validate().unwrap();
    (completed, value)
}

async fn seed(store: &MongoHomeBackupStore) -> Vec<HomeBackup> {
    let mut rows = Vec::new();
    for (epoch, at) in [(91, "00:00:00"), (92, "15:00:00"), (93, "15:30:00")] {
        let mut value = standalone_backup(HomeBackupState::Ready);
        value.epoch = epoch;
        value.id = format!("hb-{}-{epoch}", value.sandbox_id);
        value.cut.ready_at = Some(format!("2026-11-16T{at}Z"));
        value.retention.expires_at = "2026-12-16T00:00:00Z".into();
        if epoch == 91 {
            value.state = HomeBackupState::Deleted;
            value.erasure = Some(HomeBackupErasure {
                erased_at: "2026-11-16T00:01:00Z".into(),
                receipt: "historical-deletion-fixture".into(),
            });
        }
        store
            .backups
            .insert_one(backup_document(&value).unwrap())
            .await
            .unwrap();
        rows.push(value);
    }
    rows
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn ready_publication_atomically_applies_policy_and_short_successor_without_promoting_deleted_history(
) {
    let fixture = Fixture::new().await;
    let previous = seed(&fixture.store).await;
    let claim = creating(&fixture.store).await;
    let (schedule, value) = ready(&claim, "2026-11-16T15:45:00Z");
    let done = fixture
        .store
        .save_capture(&claim, &schedule, &value, &layer2_core::now_rfc3339())
        .await
        .unwrap();
    let stored = done.backup.unwrap();
    assert_eq!(stored.retention.class, HomeBackupRetentionClass::Short);
    assert_eq!(stored.retention.expires_at, "2026-12-16T15:45:00Z");
    let hourly = fixture
        .store
        .get_backup(&previous[1].id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(hourly.retention.class, HomeBackupRetentionClass::Hourly);
    assert_eq!(hourly.retention.expires_at, "2026-11-23T15:00:00Z");
    let short = fixture
        .store
        .get_backup(&previous[2].id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(short.retention.class, HomeBackupRetentionClass::Short);
    assert_eq!(short.retention.expires_at, "2026-11-16T17:45:00Z");
    assert_eq!(
        fixture.store.get_backup(&previous[0].id).await.unwrap(),
        Some(previous[0].clone())
    );
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn lost_schedule_authority_prevents_all_retention_and_ready_publication() {
    let fixture = Fixture::new().await;
    let previous = seed(&fixture.store).await;
    let claim = creating(&fixture.store).await;
    let mut stolen = claim.schedule.clone();
    stolen.next_fence += 1;
    stolen.revision += 1;
    stolen.lease.as_mut().unwrap().fence += 1;
    stolen.lease.as_mut().unwrap().token = uuid::Uuid::new_v4().to_string();
    fixture
        .store
        .schedules
        .replace_one(doc! {"_id":&stolen.id}, schedule_document(&stolen).unwrap())
        .await
        .unwrap();
    let (schedule, value) = ready(&claim, "2026-11-16T16:00:00Z");
    assert_eq!(
        fixture
            .store
            .save_capture(&claim, &schedule, &value, &layer2_core::now_rfc3339())
            .await,
        Err(HomeBackupError::Conflict)
    );
    for row in previous {
        assert_eq!(fixture.store.get_backup(&row.id).await.unwrap(), Some(row));
    }
    assert_eq!(
        fixture.store.get_backup(&value.id).await.unwrap(),
        claim.backup
    );
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn ready_publication_crossing_sgt_midnight_becomes_daily() {
    let fixture = Fixture::new().await;
    seed(&fixture.store).await;
    let claim = creating(&fixture.store).await;
    let (schedule, value) = ready(&claim, "2026-11-16T16:00:00Z");
    let done = fixture
        .store
        .save_capture(&claim, &schedule, &value, &layer2_core::now_rfc3339())
        .await
        .unwrap();
    let stored = done.backup.unwrap();
    assert_eq!(stored.retention.class, HomeBackupRetentionClass::Daily);
    assert_eq!(stored.retention.expires_at, "2026-12-16T16:00:00Z");
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn oversized_history_aborts_ready_and_schedule_publication_instead_of_using_partial_retention(
) {
    let fixture = Fixture::new().await;
    let mut rows = Vec::new();
    for epoch in 100..4197 {
        let mut value = standalone_backup(HomeBackupState::Ready);
        value.epoch = epoch;
        value.id = format!("hb-{}-{epoch}", value.sandbox_id);
        rows.push(backup_document(&value).unwrap());
    }
    fixture.store.backups.insert_many(rows).await.unwrap();
    let claim = creating(&fixture.store).await;
    let (schedule, value) = ready(&claim, "2026-11-16T16:00:00Z");
    assert_eq!(
        fixture
            .store
            .save_capture(&claim, &schedule, &value, &layer2_core::now_rfc3339())
            .await,
        Err(HomeBackupError::Invalid(
            "ready history exceeds publication bound"
        ))
    );
    assert_eq!(
        fixture.store.get_backup(&value.id).await.unwrap(),
        claim.backup
    );
    assert_eq!(
        fixture.store.get_schedule(&schedule.id).await.unwrap(),
        Some(claim.schedule)
    );
    fixture.cleanup().await;
}
