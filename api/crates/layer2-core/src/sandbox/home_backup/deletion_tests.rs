use super::*;

fn deletion_lease() -> HomeBackupLease {
    let mut value = lease();
    value.expires_at = "2026-11-17T00:01:00Z".into();
    value
}

fn observation() -> HomeBackupDeletionObservation {
    HomeBackupDeletionObservation {
        snapshot_name: "capture-1".into(),
        source_reference: Some("handle-fixture".into()),
        absent: true,
        observed_at: "2026-11-17T00:00:10Z".into(),
        receipt: "deletion-confirmation-fixture".into(),
    }
}

#[test]
fn expiry_claim_persists_expiring_before_deletion_and_fences_takeover() {
    let original = backup(HomeBackupState::Ready);
    let claimed = original
        .claim_deletion(
            HomeBackupDeletionMode::Expiry,
            "2026-11-17T00:00:00Z",
            deletion_lease(),
        )
        .unwrap();
    assert_eq!(original.state, HomeBackupState::Ready);
    assert_eq!(claimed.state, HomeBackupState::Expiring);
    assert_eq!((claimed.revision, claimed.next_fence), (2, 2));
    assert!(claimed
        .claim_deletion(
            HomeBackupDeletionMode::Expiry,
            "2026-11-17T00:00:30Z",
            deletion_lease()
        )
        .is_err());
    let mut next_lease = deletion_lease();
    next_lease.fence = 2;
    next_lease.expires_at = "2026-11-17T00:03:00Z".into();
    let takeover = claimed
        .claim_deletion(
            HomeBackupDeletionMode::Expiry,
            "2026-11-17T00:01:00Z",
            next_lease,
        )
        .unwrap();
    assert_eq!((takeover.revision, takeover.next_fence), (3, 3));
    assert_eq!(takeover.lease.unwrap().fence, 2);
}

#[test]
fn pinned_unexpired_active_capture_and_deleted_backups_refuse_expiry() {
    let mut pinned = backup(HomeBackupState::Ready);
    pinned.restore_pins = 1;
    assert!(pinned
        .claim_deletion(
            HomeBackupDeletionMode::Expiry,
            "2026-11-17T00:00:00Z",
            deletion_lease()
        )
        .is_err());
    assert!(backup(HomeBackupState::Ready)
        .claim_deletion(
            HomeBackupDeletionMode::Expiry,
            "2026-11-16T23:59:59Z",
            deletion_lease()
        )
        .is_err());
    for state in [
        HomeBackupState::Requested,
        HomeBackupState::Creating,
        HomeBackupState::Deleted,
    ] {
        assert!(backup(state)
            .claim_deletion(
                HomeBackupDeletionMode::Expiry,
                "2026-11-17T00:00:00Z",
                deletion_lease()
            )
            .is_err());
    }
}

#[test]
fn erasure_can_claim_cancelled_pending_and_unexpired_backups_but_never_pinned_ones() {
    for state in [
        HomeBackupState::Requested,
        HomeBackupState::Creating,
        HomeBackupState::Ready,
        HomeBackupState::Failed,
    ] {
        assert!(backup(state)
            .claim_deletion(
                HomeBackupDeletionMode::Erasure,
                "2026-11-16T00:00:00Z",
                deletion_lease()
            )
            .is_ok());
    }
    let mut pinned = backup(HomeBackupState::Ready);
    pinned.restore_pins = 1;
    assert!(pinned
        .claim_deletion(
            HomeBackupDeletionMode::Erasure,
            "2026-11-16T00:00:00Z",
            deletion_lease()
        )
        .is_err());
}

#[test]
fn deletion_requires_independent_absence_for_the_exact_target_and_a_live_lease() {
    let claimed = backup(HomeBackupState::Ready)
        .claim_deletion(
            HomeBackupDeletionMode::Expiry,
            "2026-11-17T00:00:00Z",
            deletion_lease(),
        )
        .unwrap();
    for change in ["present", "name", "reference", "future", "receipt"] {
        let mut proof = observation();
        match change {
            "present" => proof.absent = false,
            "name" => proof.snapshot_name.push_str("-replaced"),
            "reference" => proof.source_reference = None,
            "future" => proof.observed_at = "2026-11-17T00:00:31Z".into(),
            "receipt" => proof.receipt.clear(),
            _ => unreachable!(),
        }
        assert!(
            claimed
                .complete_deletion(&proof, "2026-11-17T00:00:30Z")
                .is_err(),
            "{change}"
        );
    }
    assert!(claimed
        .complete_deletion(&observation(), "2026-11-17T00:01:00Z")
        .is_err());
    let completed = claimed
        .complete_deletion(&observation(), "2026-11-17T00:00:30Z")
        .unwrap();
    assert_eq!(completed.state, HomeBackupState::Deleted);
    assert_eq!(completed.revision, 3);
    assert!(completed.lease.is_none());
    assert_eq!(
        completed.erasure.unwrap().erased_at,
        observation().observed_at
    );
}

#[test]
fn failed_capture_deletion_preserves_missing_evidence_and_invalid_claims_are_atomic() {
    let mut failed = backup(HomeBackupState::Failed);
    failed.next_fence = i64::MAX as u64;
    let original = failed.clone();
    let mut authority = deletion_lease();
    authority.fence = failed.next_fence;
    assert_eq!(
        failed.claim_deletion(
            HomeBackupDeletionMode::Expiry,
            "2026-11-17T00:00:00Z",
            authority
        ),
        Err(HomeBackupError::Exhausted)
    );
    assert_eq!(failed, original);
    let claimed = backup(HomeBackupState::Failed)
        .claim_deletion(
            HomeBackupDeletionMode::Expiry,
            "2026-11-17T00:00:00Z",
            deletion_lease(),
        )
        .unwrap();
    let mut proof = observation();
    proof.source_reference = None;
    let deleted = claimed
        .complete_deletion(&proof, "2026-11-17T00:00:30Z")
        .unwrap();
    assert!(deleted.cut.ready_at.is_none());
    assert!(deleted.storage.source_reference.is_none());
    assert_eq!(deleted.failure, Some(HomeBackupFailure::CopyTimedOut));
}
