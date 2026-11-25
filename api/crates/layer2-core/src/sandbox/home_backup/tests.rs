use super::*;

#[path = "deletion_tests.rs"]
mod deletion;
#[path = "orphan_tests.rs"]
mod orphan;

fn lease() -> HomeBackupLease {
    HomeBackupLease {
        holder: format!("fixture:{}", uuid::Uuid::nil()),
        token: uuid::Uuid::nil().to_string(),
        fence: 1,
        expires_at: "2026-11-16T00:01:00Z".into(),
    }
}
fn schedule() -> HomeBackupSchedule {
    HomeBackupSchedule {
        id: "sandbox-fixture".into(),
        home: HomeBackupHome {
            mode: "full_home".into(),
            home_source_id: "source-fixture".into(),
            home_identity: "home-fixture".into(),
            source_reference: Some("home-fixture".into()),
        },
        state: HomeBackupScheduleState::Active,
        next_due_at: "2026-11-16T00:00:00Z".into(),
        last_ready_at: None,
        last_ready_backup_id: None,
        lease: None,
        next_fence: 1,
        in_flight: None,
        revision: 1,
    }
}
#[test]
fn schedule_claim_advances_revision_and_monotonic_fence() {
    let mut schedule = schedule();
    schedule.claim("2026-11-16T00:00:00Z", lease()).unwrap();
    assert_eq!((schedule.revision, schedule.next_fence), (2, 2));
    assert_eq!(schedule.lease.as_ref().unwrap().fence, 1);
    assert_eq!(
        schedule.claim("2026-11-16T00:00:30Z", lease()),
        Err(HomeBackupError::Conflict)
    );
}
#[test]
fn claim_rejects_bad_clock_retired_schedule_and_exhausted_counter() {
    let mut schedule = schedule();
    assert!(schedule.claim("2026-11-16", lease()).is_err());
    schedule.state = HomeBackupScheduleState::Retiring;
    assert_eq!(
        schedule.claim("2026-11-16T00:00:00Z", lease()),
        Err(HomeBackupError::Conflict)
    );
    assert_eq!(increment(i64::MAX as u64), Err(HomeBackupError::Exhausted));
    assert_eq!(increment(u64::MAX), Err(HomeBackupError::Exhausted));
}
#[test]
fn strict_schedule_reader_rejects_unknown_missing_and_inconsistent_fields() {
    let original = serde_json::to_value(schedule()).unwrap();
    assert_eq!(
        serde_json::from_value::<HomeBackupSchedule>(original.clone()).unwrap(),
        schedule()
    );
    let mut unknown = original.clone();
    unknown["owner"] = serde_json::json!("unexpected");
    assert!(serde_json::from_value::<HomeBackupSchedule>(unknown).is_err());
    let mut missing = original;
    missing.as_object_mut().unwrap().remove("revision");
    assert!(serde_json::from_value::<HomeBackupSchedule>(missing).is_err());
    let mut bad = schedule();
    bad.last_ready_at = Some("2026-11-16T00:00:00Z".into());
    assert!(bad.validate().is_err());
    bad.last_ready_at = None;
    bad.lease = Some(lease());
    assert!(bad.validate().is_err());
}

fn backup(state: HomeBackupState) -> HomeBackup {
    let mut value = HomeBackup {
        id: "hb-sandbox-fixture-1".into(),
        sandbox_id: "sandbox-fixture".into(),
        epoch: 1,
        source: HomeBackupSource {
            home_source_id: "source-fixture".into(),
            home_identity: "home-fixture".into(),
            runtime_generation: 1,
            runtime_version: "fixture-image".into(),
            boot_id: "fixture-boot".into(),
        },
        cut: HomeBackupCut {
            requested_at: "2026-11-16T00:00:00Z".into(),
            source_creation_time: None,
            ready_at: None,
        },
        storage: HomeBackupStorage {
            snapshot_name: "capture-1".into(),
            snapshot_class: "retain-fixture".into(),
            source_reference: None,
            restore_size_bytes: None,
        },
        custody: HomeBackupCustody {
            encryption_key_version_id: Some("key-version-fixture".into()),
            auth_key_ids: vec!["auth-version-fixture".into()],
        },
        retention: HomeBackupRetention {
            class: HomeBackupRetentionClass::Daily,
            expires_at: "2026-11-17T00:00:00Z".into(),
        },
        state,
        restore_pins: 0,
        lease: None,
        next_fence: 1,
        revision: 1,
        failure: None,
        erasure: None,
    };
    match state {
        HomeBackupState::Failed => value.failure = Some(HomeBackupFailure::CopyTimedOut),
        HomeBackupState::Ready => {
            value.cut.source_creation_time = Some(value.cut.requested_at.clone());
            value.cut.ready_at = Some(value.cut.requested_at.clone());
            value.storage.source_reference = Some("handle-fixture".into());
        }
        HomeBackupState::Deleted => {
            value.erasure = Some(HomeBackupErasure {
                erased_at: "2026-11-16T00:00:01Z".into(),
                receipt: "receipt-fixture".into(),
            });
        }
        _ => {}
    }
    value
}

#[test]
fn all_states_validate_including_unready_failure_expiry_deletion_chain() {
    for state in [
        HomeBackupState::Requested,
        HomeBackupState::Creating,
        HomeBackupState::Ready,
        HomeBackupState::Failed,
        HomeBackupState::Expiring,
        HomeBackupState::Deleted,
    ] {
        assert!(backup(state).validate().is_ok(), "state {state:?}");
    }
    let failed = backup(HomeBackupState::Failed);
    assert!(failed.cut.ready_at.is_none());
    assert!(failed.storage.source_reference.is_none());
    let mut expiring = failed.clone();
    expiring.state = HomeBackupState::Expiring;
    assert!(expiring.validate().is_ok());
    let mut deleted = expiring;
    deleted.state = HomeBackupState::Deleted;
    deleted.erasure = Some(HomeBackupErasure {
        erased_at: "2026-11-16T00:00:01Z".into(),
        receipt: "receipt-fixture".into(),
    });
    assert!(deleted.validate().is_ok());
}

#[test]
fn ready_requires_evidence_but_deletion_does_not_fabricate_it() {
    let mut ready = backup(HomeBackupState::Ready);
    ready.cut.ready_at = None;
    assert!(ready.validate().is_err());
    let mut expiring = backup(HomeBackupState::Failed);
    expiring.state = HomeBackupState::Expiring;
    assert!(expiring.validate().is_ok());
}
