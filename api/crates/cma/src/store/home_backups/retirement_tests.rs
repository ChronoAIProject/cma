use super::*;

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn retirement_fences_the_dispatched_holder_without_extending_its_drain_deadline() {
    let fixture = Fixture::new().await;
    let store = &fixture.store;
    let previous_ready = complete_previous_ready(store).await;
    let now = layer2_core::now_rfc3339();
    let claimed = store
        .claim_due(&now, &holder(), &until())
        .await
        .unwrap()
        .unwrap();
    let planned = store
        .plan_capture(&claimed, &backup(&claimed), &now)
        .await
        .unwrap();
    let retired = store.retire_schedule(&schedule()).await.unwrap();
    assert_eq!(retired.state, HomeBackupScheduleState::Retiring);
    assert!(retired.next_fence > planned.schedule.next_fence);
    assert_eq!(retired.lease, planned.schedule.lease);
    assert_eq!(retired.in_flight, planned.schedule.in_flight);
    assert_eq!(
        retired.last_ready_backup_id.as_deref(),
        Some(previous_ready.id.as_str())
    );
    assert_eq!(store.retire_schedule(&schedule()).await.unwrap(), retired);
    assert!(store
        .claim_due(&now, &holder(), &until())
        .await
        .unwrap()
        .is_none());
    let mut creating = planned.backup.clone().unwrap();
    creating.state = HomeBackupState::Creating;
    let mut progress = planned.schedule.clone();
    progress.in_flight.as_mut().unwrap().phase = HomeBackupPhase::Creating;
    assert_eq!(
        store
            .save_capture(&planned, &progress, &creating, &now)
            .await,
        Err(HomeBackupError::Conflict)
    );
    assert_eq!(
        store.get_backup(&creating.id).await.unwrap(),
        planned.backup
    );
    assert_eq!(
        store.get_schedule(&retired.id).await.unwrap(),
        Some(retired)
    );
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn retirement_sentinel_blocks_late_discovery_and_rejects_replacement_home() {
    let fixture = Fixture::new().await;
    let store = &fixture.store;
    // Discovery arriving after the retirement sentinel cannot overwrite it.
    let retired = store.retire_schedule(&schedule()).await.unwrap();
    store.ensure_schedule(&schedule()).await.unwrap();
    assert_eq!(
        store.get_schedule(&retired.id).await.unwrap(),
        Some(retired.clone())
    );
    assert!(store
        .claim_due(&layer2_core::now_rfc3339(), &holder(), &until())
        .await
        .unwrap()
        .is_none());
    let mut replaced = schedule();
    replaced.home.home_source_id.push_str("-replacement");
    assert_eq!(
        store.retire_schedule(&replaced).await,
        Err(HomeBackupError::Conflict)
    );
    assert_eq!(
        store.ensure_schedule(&replaced).await,
        Err(HomeBackupError::Conflict)
    );
    assert_eq!(
        store.get_schedule(&retired.id).await.unwrap(),
        Some(retired)
    );
    fixture.cleanup().await;
}
