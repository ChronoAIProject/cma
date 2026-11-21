use super::*;

fn schedule() -> HomeBackupSchedule {
    HomeBackupSchedule {
        id: "retiring-fixture".into(),
        home: HomeBackupHome {
            mode: "native".into(),
            home_source_id: "source-original".into(),
            home_identity: "identity-original".into(),
            source_reference: Some("home-original".into()),
        },
        state: HomeBackupScheduleState::Active,
        next_due_at: "2026-11-17T00:00:00Z".into(),
        last_ready_at: Some("2026-11-16T23:00:00Z".into()),
        last_ready_backup_id: Some("previous-ready".into()),
        lease: Some(HomeBackupLease {
            holder: format!("deployment:{}", uuid::Uuid::new_v4()),
            token: uuid::Uuid::new_v4().to_string(),
            fence: 7,
            expires_at: "2026-11-17T00:01:00Z".into(),
        }),
        next_fence: 8,
        in_flight: Some(HomeBackupInFlight {
            backup_id: "current-copy".into(),
            phase: HomeBackupPhase::Creating,
            started_at: "2026-11-17T00:00:00Z".into(),
        }),
        revision: 4,
    }
}

#[test]
fn retirement_fences_capture_and_preserves_the_original_drain_deadline() {
    let before = schedule();
    let after = before.retiring().unwrap();
    assert_eq!(after.state, HomeBackupScheduleState::Retiring);
    assert_eq!(after.next_fence, before.next_fence + 1);
    assert_eq!(after.revision, before.revision + 1);
    assert_eq!(after.lease, before.lease);
    assert_eq!(after.in_flight, before.in_flight);
    assert_eq!(after.last_ready_at, before.last_ready_at);
    assert_eq!(after.last_ready_backup_id, before.last_ready_backup_id);
    assert_eq!(after.retiring().unwrap(), after);
    let mut stale = after.clone();
    assert_eq!(
        stale.claim("2026-11-17T00:02:00Z", before.lease.unwrap()),
        Err(HomeBackupError::Conflict)
    );
    assert_eq!(stale, after);
}

#[test]
fn retirement_does_not_override_other_authority_or_overflow_counters() {
    for state in [
        HomeBackupScheduleState::Restoring,
        HomeBackupScheduleState::Erasing,
        HomeBackupScheduleState::Closed,
    ] {
        let mut before = schedule();
        before.state = state;
        before.lease = None;
        before.in_flight = None;
        assert_eq!(before.retiring(), Err(HomeBackupError::Conflict));
    }
    for counter in ["revision", "next_fence"] {
        let mut before = schedule();
        if counter == "revision" {
            before.revision = i64::MAX as u64;
        } else {
            before.next_fence = i64::MAX as u64;
        }
        let original = before.clone();
        assert_eq!(before.retiring(), Err(HomeBackupError::Exhausted));
        assert_eq!(before, original);
    }
}
