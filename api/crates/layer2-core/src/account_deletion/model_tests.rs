use super::*;
use crate::SubjectId;
use std::{collections::BTreeMap, time::Duration};
const NOW: &str = "2026-11-14T00:00:00Z";
fn requested() -> AccountDeletion {
    AccountDeletion::request(AccountDeletionId::generate(), SubjectId::new("owner"), NOW).unwrap()
}
#[test]
fn erasure_order_matches_issue_1500() {
    assert_eq!(
        ERASURE_ORDER,
        [
            ErasureStep::Inventory,
            ErasureStep::RetireSandboxes,
            ErasureStep::ReleaseHomes,
            ErasureStep::EraseRecords,
            ErasureStep::EraseObjects,
            ErasureStep::RevokeCredentials,
            ErasureStep::VerifyNoResidue
        ]
    );
}
#[test]
fn deadline_is_seven_days_after_request() {
    assert_eq!(requested().deadline_at, "2026-11-21T00:00:00Z");
}
#[test]
fn advance_with_stale_fence_is_rejected() {
    let mut deletion = requested();
    let fence = deletion
        .claim("worker", NOW, Duration::from_secs(90))
        .unwrap();
    let renewed_at = "2026-11-14T00:02:00Z";
    let renewed = deletion
        .claim("other-worker", renewed_at, Duration::from_secs(90))
        .unwrap();
    assert_ne!(fence, renewed);
    let revision = deletion.revision;
    assert_eq!(
        deletion.advance(fence, renewed_at, BTreeMap::new()),
        Err(AccountDeletionError::StaleFence)
    );
    assert_eq!(deletion.revision, revision);
}
#[test]
fn blocked_rearm_resumes_same_step() {
    let mut deletion = requested();
    deletion.step = ErasureStep::EraseObjects;
    let fence = deletion
        .claim("worker", NOW, Duration::from_secs(90))
        .unwrap();
    deletion
        .block(
            fence,
            NOW,
            ErasureBlock::ObjectStoreRefused {
                reason: ErasureFailureCode::Unsupported,
            },
        )
        .unwrap();
    deletion.rearm(NOW).unwrap();
    assert_eq!(deletion.step, ErasureStep::EraseObjects);
    assert_eq!(deletion.status, AccountDeletionStatus::Running);
    assert_eq!(deletion.work.attempts, 0);
}
#[test]
fn complete_requires_verify_step_and_clears_subject() {
    let mut deletion = requested();
    let fence = deletion
        .claim("worker", NOW, Duration::from_secs(90))
        .unwrap();
    assert_eq!(
        deletion.complete(fence, NOW, Duration::from_secs(37 * 86400)),
        Err(AccountDeletionError::Invalid)
    );
    deletion.step = ErasureStep::VerifyNoResidue;
    deletion.residue = Some(ResidueReport {
        keys: KeyReceipt {
            remaining: 0,
            empty_at: Some(NOW.into()),
        },
        ..ResidueReport::default()
    });
    deletion
        .complete(fence, NOW, Duration::from_secs(37 * 86400))
        .unwrap();
    assert!(deletion.subject.is_none());
    assert!(deletion.active_subject_hash.is_none());
    assert_eq!(deletion.expire_at.as_deref(), Some("2026-12-21T00:00:00Z"));
}
#[test]
fn completion_rejects_nonzero_residue_and_expired_lease() {
    let mut deletion = requested();
    deletion.step = ErasureStep::VerifyNoResidue;
    let fence = deletion
        .claim("worker", NOW, Duration::from_secs(90))
        .unwrap();
    deletion.residue = Some(ResidueReport {
        objects: 1,
        keys: KeyReceipt {
            remaining: 0,
            empty_at: Some(NOW.into()),
        },
        ..ResidueReport::default()
    });
    assert_eq!(
        deletion.complete(fence, NOW, Duration::from_secs(37 * 86400)),
        Err(AccountDeletionError::Invalid)
    );
    assert_eq!(
        deletion.advance(fence, "2026-11-14T00:02:00Z", BTreeMap::new()),
        Err(AccountDeletionError::StaleFence)
    );
}
#[test]
fn identifiers_and_subject_hash_reuse_document_owner_namespace() {
    let deletion = requested();
    assert_eq!(deletion.id.as_str().len(), 36);
    assert!(deletion.id.as_str().starts_with("acd_"));
    assert_eq!(
        subject_hash(&SubjectId::new("owner")),
        crate::document_artifacts::digest(b"owner")
    );
}

#[test]
fn retry_keeps_the_step_and_releases_the_lease() {
    let mut deletion = requested();
    let fence = deletion
        .claim("worker", NOW, Duration::from_secs(90))
        .unwrap();
    deletion
        .retry(fence, NOW, ErasureFailureCode::Unavailable)
        .unwrap();
    assert_eq!(deletion.step, ErasureStep::Inventory);
    assert_eq!(deletion.work.attempts, 1);
    assert!(deletion.work.lease.is_none());
}
#[test]
fn rearm_and_claim_refuse_ineligible_states() {
    let mut deletion = requested();
    assert_eq!(deletion.rearm(NOW), Err(AccountDeletionError::Invalid));
    deletion
        .claim("worker", NOW, Duration::from_secs(90))
        .unwrap();
    assert_eq!(
        deletion.claim("other", NOW, Duration::from_secs(90)),
        Err(AccountDeletionError::Invalid)
    );
}
#[test]
fn missing_key_receipt_does_not_complete() {
    let mut deletion = requested();
    deletion.step = ErasureStep::VerifyNoResidue;
    deletion.residue = Some(ResidueReport::default());
    let fence = deletion
        .claim("worker", NOW, Duration::from_secs(90))
        .unwrap();
    assert_eq!(
        deletion.complete(fence, NOW, Duration::from_secs(37 * 86400)),
        Err(AccountDeletionError::Invalid)
    );
    assert!(!ResidueReport::default().is_zero());
}
#[test]
fn completion_records_the_verification_step_in_the_receipt() {
    let mut deletion = requested();
    deletion.step = ErasureStep::VerifyNoResidue;
    deletion.residue = Some(ResidueReport {
        keys: KeyReceipt {
            remaining: 0,
            empty_at: Some(NOW.into()),
        },
        ..ResidueReport::default()
    });
    let fence = deletion
        .claim("worker", NOW, Duration::from_secs(90))
        .unwrap();
    deletion
        .complete(fence, NOW, Duration::from_secs(37 * 86400))
        .unwrap();
    assert_eq!(
        deletion.receipt().steps.last().unwrap().step,
        ErasureStep::VerifyNoResidue
    );
}
#[test]
fn completed_tombstone_scrubs_inventory_identities_but_preserves_counts() {
    let mut deletion = requested();
    deletion.inventory = ErasureInventory {
        sandboxes: vec!["sandbox".into()],
        workspaces: vec!["workspace".into()],
        worktrees: vec!["worktree".into()],
        agents: vec!["agent".into()],
        launches: vec!["launch".into()],
        bindings: vec!["binding".into()],
        triggers: vec!["trigger".into()],
        bots: vec!["bot".into()],
        documents: vec!["document".into()],
        homes: vec!["home".into()],
        pending_key_ids: vec!["credential-id".into()],
        counts: BTreeMap::from([("records".into(), 4)]),
    };
    deletion.step = ErasureStep::VerifyNoResidue;
    deletion.residue = Some(ResidueReport {
        keys: KeyReceipt {
            remaining: 0,
            empty_at: Some(NOW.into()),
        },
        ..ResidueReport::default()
    });
    let fence = deletion
        .claim("worker", NOW, Duration::from_secs(90))
        .unwrap();
    deletion
        .complete(fence, NOW, Duration::from_secs(37 * 86400))
        .unwrap();
    assert_eq!(
        deletion.inventory,
        ErasureInventory {
            counts: BTreeMap::from([("records".into(), 4)]),
            ..ErasureInventory::default()
        }
    );
}
#[test]
fn unparseable_key_receipt_cannot_claim_zero() {
    let residue = ResidueReport {
        keys: KeyReceipt {
            remaining: 0,
            empty_at: Some("unverified".into()),
        },
        ..ResidueReport::default()
    };
    assert!(!residue.is_zero());
}
#[test]
fn ordered_success_advances_each_step_with_counts_and_final_verification() {
    let mut deletion = requested();
    let mut sequence = deletion.event_seq;
    for (index, step) in ERASURE_ORDER.iter().take(6).copied().enumerate() {
        assert_eq!(deletion.step, step);
        let fence = deletion
            .claim("worker", NOW, Duration::from_secs(90))
            .unwrap();
        assert_eq!(deletion.event_seq, sequence + 1);
        let counts = BTreeMap::from([("processed".into(), index as u64 + 1)]);
        let event = deletion.advance(fence, NOW, counts.clone()).unwrap();
        sequence += 2;
        assert_eq!(event.seq, sequence);
        assert_eq!(event.step, step);
        assert_eq!(
            event.kind,
            ErasureEventKind::Completed {
                counts: counts.clone()
            }
        );
        assert_eq!(deletion.steps.last().unwrap().counts, counts);
        assert_eq!(deletion.step, ERASURE_ORDER[index + 1]);
        assert!(deletion.work.lease.is_none());
        assert_eq!(deletion.work.attempts, 0);
        assert_eq!(deletion.status, AccountDeletionStatus::Running);
    }
    deletion.residue = Some(ResidueReport {
        keys: KeyReceipt {
            remaining: 0,
            empty_at: Some(NOW.into()),
        },
        ..ResidueReport::default()
    });
    let fence = deletion
        .claim("worker", NOW, Duration::from_secs(90))
        .unwrap();
    let event = deletion
        .complete(fence, NOW, Duration::from_secs(37 * 86400))
        .unwrap();
    assert_eq!(event.step, ErasureStep::VerifyNoResidue);
    assert_eq!(deletion.status, AccountDeletionStatus::Completed);
    assert_eq!(
        deletion
            .receipt()
            .steps
            .iter()
            .map(|step| step.step)
            .collect::<Vec<_>>(),
        ERASURE_ORDER
    );
    assert!(!deletion.is_active());
}

#[test]
fn residue_reports_each_nonzero_class() {
    let zero = ResidueReport {
        keys: KeyReceipt {
            remaining: 0,
            empty_at: Some(NOW.into()),
        },
        ..ResidueReport::default()
    };
    assert!(zero.is_zero());
    let mut records = zero.clone();
    records.records.insert("records".into(), 1);
    let mut objects = zero.clone();
    objects.objects = 1;
    let mut homes = zero.clone();
    homes.homes.push("retained-home".into());
    let mut keys = zero.clone();
    keys.keys.remaining = 1;
    for residue in [records, objects, homes, keys] {
        assert!(!residue.is_zero());
    }
}
