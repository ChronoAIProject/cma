use super::*;
use crate::SubjectId;
use std::time::Duration;

const NOW: &str = "2026-11-14T00:00:00Z";
const RENEWED: &str = "2026-11-14T00:01:00Z";
const EXPIRED: &str = "2026-11-14T00:02:00Z";

fn claimed() -> (AccountDeletion, u64) {
    let mut record =
        AccountDeletion::request(AccountDeletionId::generate(), SubjectId::new("owner"), NOW)
            .unwrap();
    let fence = record
        .claim("worker", NOW, Duration::from_secs(90))
        .unwrap();
    (record, fence)
}

#[test]
fn renewal_changes_only_revision_and_expiry() {
    let (mut record, fence) = claimed();
    record.work.attempts = 3;
    let mut expected = record.clone();
    expected.revision += 1;
    expected.work.lease.as_mut().unwrap().expires_at = "2026-11-14T00:02:30Z".into();
    record
        .renew(fence, RENEWED, Duration::from_secs(90))
        .unwrap();
    assert_eq!(record, expected);
}

#[test]
fn renewal_rejects_stale_fence_expired_lease_and_zero_duration() {
    let (record, fence) = claimed();
    for (used, at, duration, error) in [
        (
            fence + 1,
            RENEWED,
            Duration::from_secs(90),
            AccountDeletionError::StaleFence,
        ),
        (
            fence,
            EXPIRED,
            Duration::from_secs(90),
            AccountDeletionError::StaleFence,
        ),
        (
            fence,
            RENEWED,
            Duration::ZERO,
            AccountDeletionError::Invalid,
        ),
    ] {
        let mut attempt = record.clone();
        assert_eq!(attempt.renew(used, at, duration), Err(error));
        assert_eq!(attempt, record);
    }
}

#[test]
fn defer_waits_without_failure_or_audit_event() {
    let (mut record, fence) = claimed();
    record.work.attempts = 3;
    let mut expected = record.clone();
    expected.revision += 1;
    expected.work.ready_at = EXPIRED.into();
    expected.work.lease = None;
    record.defer(fence, EXPIRED).unwrap();
    assert_eq!(record, expected);
    assert!(!record.is_due(RENEWED));
    assert!(record.is_due(EXPIRED));
}

#[test]
fn defer_refuses_stale_fence_and_invalid_schedule_without_mutation() {
    let (record, fence) = claimed();
    let mut stale = record.clone();
    assert_eq!(
        stale.defer(fence + 1, EXPIRED),
        Err(AccountDeletionError::StaleFence)
    );
    assert_eq!(stale, record);
    let mut invalid = record.clone();
    assert_eq!(
        invalid.defer(fence, "invalid"),
        Err(AccountDeletionError::Invalid)
    );
    assert_eq!(invalid, record);
}

#[test]
fn retry_records_a_closed_failure_code() {
    let (mut record, fence) = claimed();
    let event = record
        .retry(fence, NOW, ErasureFailureCode::Unavailable)
        .unwrap();
    assert_eq!(
        event.kind,
        ErasureEventKind::Retried {
            failure: ErasureFailureCode::Unavailable
        }
    );
    assert!(serde_json::from_value::<ErasureEventKind>(
        serde_json::json!({"kind":"retried","failure":"user-authored text"})
    )
    .is_err());
    assert!(serde_json::from_value::<ErasureBlock>(
        serde_json::json!({"kind":"object_store_refused","reason":"private object path"})
    )
    .is_err());
    let typed = ErasureBlock::ObjectStoreRefused {
        reason: ErasureFailureCode::Unsupported,
    };
    assert_eq!(
        serde_json::from_str::<ErasureBlock>(&serde_json::to_string(&typed).unwrap()).unwrap(),
        typed
    );
}

#[test]
fn blocked_events_hash_resource_identifiers_before_persistence() {
    for reason in [
        ErasureBlock::RetirementArchiveHold {
            sandbox_id: "private-sandbox-id".into(),
        },
        ErasureBlock::SandboxRetirementFailed {
            sandbox_id: "private-sandbox-id".into(),
        },
        ErasureBlock::HomeNotFinal {
            sandbox_id: "private-sandbox-id".into(),
        },
    ] {
        let (mut record, fence) = claimed();
        let event = record.block(fence, NOW, reason).unwrap();
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains("private-sandbox-id"));
        assert!(json.contains(&crate::document_artifacts::digest(b"private-sandbox-id")));
    }
}

#[test]
fn completion_clears_remaining_block_identity() {
    let (mut record, fence) = claimed();
    record.step = ErasureStep::VerifyNoResidue;
    record.block = Some(ErasureBlock::HomeNotFinal {
        sandbox_id: "private-sandbox-id".into(),
    });
    record.residue = Some(ResidueReport {
        keys: KeyReceipt {
            remaining: 0,
            empty_at: Some(NOW.into()),
        },
        ..ResidueReport::default()
    });
    record
        .complete(fence, NOW, Duration::from_secs(37 * 86400))
        .unwrap();
    assert!(record.block.is_none());
    assert!(!serde_json::to_string(&record)
        .unwrap()
        .contains("private-sandbox-id"));
}
