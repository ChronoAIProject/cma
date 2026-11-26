use super::*;

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn dual_holder_claims_and_catalog_publication_are_transactionally_fenced() {
    let fixture = Fixture::new().await;
    let store = &fixture.store;
    assert_eq!(
        store.schedules.read_concern().unwrap().level,
        ReadConcernLevel::Majority
    );
    assert_eq!(
        store.backups.write_concern().unwrap().w,
        Some(Acknowledgment::Majority)
    );
    store.ensure_schedule(&schedule()).await.unwrap();
    let now = layer2_core::now_rfc3339();
    let until = until();
    let first_holder = holder();
    let second_holder = holder();
    let (a, b) = tokio::join!(
        store.claim_due(&now, &first_holder, &until),
        store.claim_due(&now, &second_holder, &until)
    );
    let mut winners = [a, b]
        .into_iter()
        .filter_map(|result| match result {
            Ok(value) => value,
            Err(HomeBackupError::Conflict) => None,
            Err(error) => panic!("unexpected claim failure: {error}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(winners.len(), 1);
    let claim = winners.pop().unwrap();
    let intent = backup(&claim);
    let planned = store.plan_capture(&claim, &intent, &now).await.unwrap();
    assert!(matches!(
        store.plan_capture(&claim, &intent, &now).await,
        Err(HomeBackupError::Conflict)
    ));
    let mut creating = planned.backup.clone().unwrap();
    creating.state = HomeBackupState::Creating;
    let mut progress = planned.schedule.clone();
    progress.in_flight.as_mut().unwrap().phase = HomeBackupPhase::Creating;
    let creating_claim = store
        .save_capture(&planned, &progress, &creating, &now)
        .await
        .unwrap();
    assert!(matches!(
        store
            .save_capture(&planned, &progress, &creating, &now)
            .await,
        Err(HomeBackupError::Conflict)
    ));
    let mut ready = creating_claim.backup.clone().unwrap();
    ready.state = HomeBackupState::Ready;
    ready.cut.source_creation_time = Some(ready.cut.requested_at.clone());
    ready.cut.ready_at = Some(ready.cut.requested_at.clone());
    ready.storage.source_reference = Some("handle-fixture".into());
    let mut completed = creating_claim.schedule.clone();
    completed.in_flight = None;
    completed.last_ready_at = ready.cut.ready_at.clone();
    completed.last_ready_backup_id = Some(ready.id.clone());
    completed.next_due_at = until.clone();
    let done = store
        .save_capture(&creating_claim, &completed, &ready, &now)
        .await
        .unwrap();
    assert!(done.schedule.lease.is_none());
    assert!(done.backup.as_ref().unwrap().lease.is_none());
    assert_eq!(store.get_backup(&ready.id).await.unwrap(), done.backup);
    assert_eq!(
        store.get_schedule(&completed.id).await.unwrap(),
        Some(done.schedule)
    );
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn expired_and_stolen_document_leases_cannot_publish_or_insert_catalog_rows() {
    let fixture = Fixture::new().await;
    let store = &fixture.store;
    store.ensure_schedule(&schedule()).await.unwrap();
    let now = layer2_core::now_rfc3339();
    let claim = store
        .claim_due(&now, &holder(), &until())
        .await
        .unwrap()
        .unwrap();
    let intent = backup(&claim);
    let mut expired = claim.clone();
    expired.schedule.lease.as_mut().unwrap().expires_at = "2000-01-01T00:00:00Z".into();
    store
        .schedules
        .replace_one(
            doc! {"_id":&expired.schedule.id},
            schedule_document(&expired.schedule).unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        store.plan_capture(&claim, &intent, &now).await,
        Err(HomeBackupError::Conflict)
    ));
    assert!(store.get_backup(&intent.id).await.unwrap().is_none());
    let successor = store
        .claim_due(&now, &holder(), &until())
        .await
        .unwrap()
        .unwrap();
    assert!(
        successor.schedule.lease.as_ref().unwrap().fence
            > claim.schedule.lease.as_ref().unwrap().fence
    );
    let intent = backup(&successor);
    let planned = store.plan_capture(&successor, &intent, &now).await.unwrap();
    let catalog = planned.backup.as_ref().unwrap();
    store
        .backups
        .update_one(
            doc! {"_id":&catalog.id},
            doc! {"$set":{"lease.expires_at":"2000-01-01T00:00:00Z"}},
        )
        .await
        .unwrap();
    let mut creating = catalog.clone();
    creating.state = HomeBackupState::Creating;
    assert!(matches!(
        store
            .save_capture(&planned, &planned.schedule, &creating, &now)
            .await,
        Err(HomeBackupError::Conflict)
    ));
    assert_eq!(
        store.get_schedule(&planned.schedule.id).await.unwrap(),
        Some(planned.schedule)
    );
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn expired_in_flight_claim_renews_both_leases_without_creating_a_second_capture() {
    let fixture = Fixture::new().await;
    let store = &fixture.store;
    store.ensure_schedule(&schedule()).await.unwrap();
    let now = layer2_core::now_rfc3339();
    let first = store
        .claim_due(&now, &holder(), &until())
        .await
        .unwrap()
        .unwrap();
    let intent = backup(&first);
    let planned = store.plan_capture(&first, &intent, &now).await.unwrap();
    let old_schedule_fence = planned.schedule.lease.as_ref().unwrap().fence;
    let old_backup_fence = planned
        .backup
        .as_ref()
        .unwrap()
        .lease
        .as_ref()
        .unwrap()
        .fence;
    store
        .schedules
        .update_one(
            doc! {"_id":&planned.schedule.id},
            doc! {"$set":{"lease.expires_at":"2000-01-01T00:00:00Z"}},
        )
        .await
        .unwrap();
    store
        .backups
        .update_one(
            doc! {"_id":&intent.id},
            doc! {"$set":{"lease.expires_at":"2000-01-01T00:00:00Z"}},
        )
        .await
        .unwrap();
    let successor = store
        .claim_due(&now, &holder(), &until())
        .await
        .unwrap()
        .unwrap();
    let successor_backup = successor.backup.as_ref().unwrap();
    assert_eq!(successor_backup.id, intent.id);
    assert_eq!(store.backups.count_documents(doc! {}).await.unwrap(), 1);
    assert!(successor.schedule.lease.as_ref().unwrap().fence > old_schedule_fence);
    assert!(successor_backup.lease.as_ref().unwrap().fence > old_backup_fence);
    let mut stale_schedule = planned.schedule.clone();
    stale_schedule.in_flight.as_mut().unwrap().phase = HomeBackupPhase::Creating;
    let mut stale_backup = planned.backup.clone().unwrap();
    stale_backup.state = HomeBackupState::Creating;
    assert!(matches!(
        store
            .save_capture(&planned, &stale_schedule, &stale_backup, &now)
            .await,
        Err(HomeBackupError::Conflict)
    ));
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn capture_rejects_illegal_transitions_and_immutable_intent_changes_and_closes_failed() {
    let fixture = Fixture::new().await;
    let store = &fixture.store;
    let previous_ready = complete_previous_ready(store).await;
    let now = layer2_core::now_rfc3339();
    let claim = store
        .claim_due(&now, &holder(), &until())
        .await
        .unwrap()
        .unwrap();
    let intent = backup(&claim);
    let planned = store.plan_capture(&claim, &intent, &now).await.unwrap();
    let mut illegal = planned.backup.clone().unwrap();
    illegal.state = HomeBackupState::Ready;
    let mut illegal_schedule = planned.schedule.clone();
    illegal_schedule.in_flight = None;
    illegal_schedule.last_ready_at = Some(illegal.cut.requested_at.clone());
    illegal_schedule.last_ready_backup_id = Some(illegal.id.clone());
    assert!(matches!(
        store
            .save_capture(&planned, &illegal_schedule, &illegal, &now)
            .await,
        Err(HomeBackupError::Conflict)
    ));
    let mut changed = planned.backup.clone().unwrap();
    changed.storage.snapshot_name.push_str("-tampered");
    changed.state = HomeBackupState::Creating;
    assert!(matches!(
        store
            .save_capture(&planned, &planned.schedule, &changed, &now)
            .await,
        Err(HomeBackupError::Conflict)
    ));
    let mut failed = planned.backup.clone().unwrap();
    failed.state = HomeBackupState::Failed;
    failed.failure = Some(HomeBackupFailure::CopyTimedOut);
    let mut completed = planned.schedule.clone();
    completed.in_flight = None;
    completed.next_due_at = until();
    let mut cleared = completed.clone();
    cleared.last_ready_at = None;
    cleared.last_ready_backup_id = None;
    assert!(matches!(
        store.save_capture(&planned, &cleared, &failed, &now).await,
        Err(HomeBackupError::Conflict)
    ));
    let done = store
        .save_capture(&planned, &completed, &failed, &now)
        .await
        .unwrap();
    assert!(done.schedule.lease.is_none());
    assert!(done.backup.as_ref().unwrap().lease.is_none());
    assert_eq!(
        done.schedule.last_ready_backup_id.as_deref(),
        Some(previous_ready.id.as_str())
    );
    assert_eq!(
        done.schedule.last_ready_at.as_deref(),
        previous_ready.cut.ready_at.as_deref()
    );
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn exhausted_fence_and_unknown_schema_fail_without_mutation() {
    let fixture = Fixture::new().await;
    let store = &fixture.store;
    let mut exhausted = schedule();
    exhausted.next_fence = i64::MAX as u64;
    store
        .ensure_schedule(&HomeBackupSchedule {
            next_fence: 1,
            ..exhausted.clone()
        })
        .await
        .unwrap();
    store
        .schedules
        .replace_one(
            doc! {"_id":&exhausted.id},
            schedule_document(&exhausted).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .claim_due(&layer2_core::now_rfc3339(), &holder(), &until())
            .await,
        Err(HomeBackupError::Exhausted)
    );
    assert_eq!(
        store.get_schedule(&exhausted.id).await.unwrap(),
        Some(exhausted)
    );
    store
        .backups
        .insert_one(doc! {"_id":"future","document_schema_version":99})
        .await
        .unwrap();
    let reopened = MongoHomeBackupStore::open(&fixture.client, &fixture.database)
        .await
        .unwrap();
    assert!(matches!(
        reopened.get_backup("future").await,
        Err(HomeBackupError::Invalid(_))
    ));
    assert_eq!(
        store.backups.find_one(doc! {"_id":"future"}).await.unwrap(),
        Some(doc! {"_id":"future","document_schema_version":99})
    );
    fixture.cleanup().await;
}
