use super::*;
use LedgerCollectionCheckpointMutation as M;

fn initial() -> LedgerCollectionCheckpointV2 {
    LedgerCollectionCheckpointV2::new("a".repeat(64), 3, 7, "b".repeat(64), 10, 20).unwrap()
}
fn page(revision: u64, groups: Vec<String>, complete: bool) -> LedgerCollectionPageProgress {
    LedgerCollectionPageProgress {
        page_sha256: "c".repeat(64),
        active_revision: revision,
        cursor: None,
        next_cursor: (!complete).then(|| "page-64".into()),
        complete,
        remaining_groups: groups,
    }
}
fn reference(key: &str) -> LedgerCollectionArtifactReferenceV1 {
    LedgerCollectionArtifactReferenceV1 {
        format: "runtime-ledger-collection-reference-v1".into(),
        archive_id: uuid::Uuid::from_u128(1).to_string(),
        archive_version: 3,
        operation_id: uuid::Uuid::from_u128(2).to_string(),
        group_key: key.into(),
        binding_sha256: "a".repeat(64),
        plan_sha256: "d".repeat(64),
        artifacts_sha256: "e".repeat(64),
        artifact_lengths: [100, 100, 100],
    }
}
fn pending(key: &str) -> LedgerCollectionPending {
    LedgerCollectionPending {
        operation_id: uuid::Uuid::from_u128(2).to_string(),
        group_key: key.into(),
        request_sha256: "f".repeat(64),
        source_sha256: "1".repeat(64),
        observation_sha256: "2".repeat(64),
        phase: LedgerCollectionPhase::Selected,
        receipt_sha256: None,
    }
}
fn staged() -> LedgerCollectionCheckpointV2 {
    initial()
        .apply(M::PinPage {
            fence: 1,
            now: 11,
            page: page(7, vec!["3".repeat(64), "4".repeat(64)], true),
        })
        .unwrap()
        .apply(M::RecordReference {
            fence: 1,
            now: 11,
            reference: reference(&"3".repeat(64)),
        })
        .unwrap()
}
fn selected() -> LedgerCollectionCheckpointV2 {
    staged()
        .apply(M::Select {
            fence: 1,
            now: 11,
            pending: pending(&"3".repeat(64)),
        })
        .unwrap()
}
fn ack() -> LedgerCollectionFinalAcknowledgement {
    LedgerCollectionFinalAcknowledgement {
        operation_id: uuid::Uuid::from_u128(2).to_string(),
        group_key: "3".repeat(64),
        binding_sha256: "a".repeat(64),
        plan_sha256: "d".repeat(64),
        source_sha256: "1".repeat(64),
        observation_sha256: "2".repeat(64),
        before_revision: 7,
        after_revision: 8,
        receipt_sha256: "5".repeat(64),
    }
}
fn acknowledged() -> LedgerCollectionCheckpointV2 {
    selected()
        .apply(M::Acknowledge {
            fence: 1,
            now: 11,
            acknowledgement: ack(),
        })
        .unwrap()
}

#[test]
fn checkpoint_restart_and_takeover_preserve_reference_before_and_after_selection() {
    for state in [staged(), selected(), acknowledged()] {
        let bytes = serde_json::to_vec(&state).unwrap();
        let reopened: LedgerCollectionCheckpointV2 = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(reopened, state);
        assert!(serde_json::from_slice::<LedgerCollectorProgressV1>(&bytes).is_err());
        let takeover = reopened
            .apply(M::Claim {
                holder_sha256: "6".repeat(64),
                now: 20,
                expires_at: 30,
            })
            .unwrap();
        assert_eq!(takeover.accepted_artifact, state.accepted_artifact);
        assert_eq!(takeover.progress.pending, state.progress.pending);
        assert_eq!(takeover.acknowledged_revision, state.acknowledged_revision);
        assert_eq!(takeover.progress.lease.fence, 2);
        assert!(takeover
            .apply(M::Refresh {
                fence: 2,
                now: 21,
                active_revision: 8
            })
            .is_err());
        assert!(takeover
            .apply(M::Advance {
                fence: 2,
                now: 21,
                active_revision: 7,
                page_sha256: "c".repeat(64)
            })
            .is_err());
        assert!(takeover
            .apply(M::Select {
                fence: 1,
                now: 21,
                pending: pending(&"3".repeat(64))
            })
            .is_err());
    }
}

#[test]
fn checkpoint_cannot_replace_original_reference_or_select_without_it() {
    let state = staged();
    assert_eq!(
        state
            .apply(M::RecordReference {
                fence: 1,
                now: 11,
                reference: reference(&"3".repeat(64))
            })
            .unwrap(),
        state
    );
    for field in 0..9 {
        let mut changed = reference(&"3".repeat(64));
        match field {
            0 => changed.archive_id = uuid::Uuid::from_u128(3).to_string(),
            1 => changed.archive_version = 2,
            2 => changed.operation_id = uuid::Uuid::from_u128(4).to_string(),
            3 => changed.group_key = "4".repeat(64),
            4 => changed.binding_sha256 = "6".repeat(64),
            5 => changed.plan_sha256 = "7".repeat(64),
            6 => changed.artifacts_sha256 = "8".repeat(64),
            7 => changed.artifact_lengths[0] += 1,
            _ => changed.format.push('x'),
        }
        assert!(state
            .apply(M::RecordReference {
                fence: 1,
                now: 11,
                reference: changed
            })
            .is_err());
    }
    let empty = initial()
        .apply(M::PinPage {
            fence: 1,
            now: 11,
            page: page(7, vec!["3".repeat(64)], true),
        })
        .unwrap();
    assert!(empty
        .apply(M::Select {
            fence: 1,
            now: 11,
            pending: pending(&"3".repeat(64))
        })
        .is_err());
    assert!(state
        .apply(M::SelectRetained {
            fence: 1,
            now: 11,
            pending: pending(&"3".repeat(64))
        })
        .is_err());
}

#[test]
fn checkpoint_final_ack_matches_every_original_field_and_refuses_changed_replay() {
    for field in 0..11 {
        let mut changed = ack();
        match field {
            0 => changed.operation_id = uuid::Uuid::from_u128(7).to_string(),
            1 => changed.binding_sha256 = "7".repeat(64),
            2 => changed.plan_sha256 = "7".repeat(64),
            3 => changed.group_key = "7".repeat(64),
            4 => changed.source_sha256 = "7".repeat(64),
            5 => changed.observation_sha256 = "7".repeat(64),
            6 => changed.before_revision = 6,
            7 => changed.after_revision = 7,
            8 => changed.receipt_sha256 = "invalid".into(),
            9 => changed.after_revision = 9,
            _ => changed.receipt_sha256 = "A".repeat(64),
        }
        assert!(selected()
            .apply(M::Acknowledge {
                fence: 1,
                now: 11,
                acknowledgement: changed
            })
            .is_err());
    }
    let state = acknowledged();
    assert_eq!(
        state
            .apply(M::Acknowledge {
                fence: 1,
                now: 11,
                acknowledgement: ack()
            })
            .unwrap(),
        state
    );
    for change_revision in [false, true] {
        let mut changed = ack();
        if change_revision {
            changed.after_revision = 9;
        } else {
            changed.receipt_sha256 = "6".repeat(64);
        }
        assert!(state
            .apply(M::Acknowledge {
                fence: 1,
                now: 11,
                acknowledgement: changed
            })
            .is_err());
    }
}

#[test]
fn checkpoint_final_settlement_atomically_advances_position_and_discards_stale_page() {
    let original = selected();
    let settle = M::SettleAcknowledged {
        fence: 1,
        now: 11,
        operation_id: ack().operation_id,
        receipt_sha256: ack().receipt_sha256,
    };
    assert!(original.apply(settle.clone()).is_err());
    let state = acknowledged();
    assert!(state
        .apply(M::SettleAcknowledged {
            fence: 1,
            now: 11,
            operation_id: ack().operation_id,
            receipt_sha256: "6".repeat(64)
        })
        .is_err());
    let settled = state.apply(settle).unwrap();
    assert_eq!(settled.progress.revision, state.progress.revision + 1);
    assert_eq!(settled.settled_after, Some("3".repeat(64)));
    assert_eq!(settled.progress.active_revision, 8);
    assert_eq!(settled.progress.last_ack_sha256, Some(ack().receipt_sha256));
    assert!(settled.progress.pending.is_none() && settled.accepted_artifact.is_none());
    assert!(settled.progress.page.is_none() && settled.progress.cursor.is_none());
    assert!(!settled.progress.complete);
    assert!(settled
        .apply(M::PinPage {
            fence: 1,
            now: 11,
            page: page(7, vec!["4".repeat(64)], true)
        })
        .is_err());
    assert!(settled
        .apply(M::PinPage {
            fence: 1,
            now: 11,
            page: page(8, vec!["3".repeat(64)], true)
        })
        .is_err());
    assert!(settled
        .apply(M::PinPage {
            fence: 1,
            now: 11,
            page: page(8, vec!["4".repeat(64)], true)
        })
        .is_ok());
}

#[test]
fn checkpoint_retained_groups_remain_fair_across_churn_and_restart() {
    let mut state = initial();
    for key in 1..=130 {
        let group = format!("{key:064x}");
        let revision = state.progress.active_revision;
        state = state
            .apply(M::PinPage {
                fence: 1,
                now: 11,
                page: page(revision, vec![group.clone()], false),
            })
            .unwrap();
        state = state
            .apply(M::SelectRetained {
                fence: 1,
                now: 11,
                pending: pending(&group),
            })
            .unwrap();
        assert!(state
            .apply(M::SettleAcknowledged {
                fence: 1,
                now: 11,
                operation_id: ack().operation_id,
                receipt_sha256: ack().receipt_sha256
            })
            .is_err());
        state = state
            .apply(M::SettleRetained {
                fence: 1,
                now: 11,
                operation_id: ack().operation_id,
            })
            .unwrap();
        assert_eq!(state.settled_after, Some(group));
        assert!(state.progress.last_ack_sha256.is_none());
        state = state
            .apply(M::Refresh {
                fence: 1,
                now: 11,
                active_revision: revision + 1,
            })
            .unwrap();
        state = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        assert!(state
            .apply(M::PinPage {
                fence: 1,
                now: 11,
                page: page(revision + 1, vec![format!("{key:064x}")], true)
            })
            .is_err());
        assert_eq!(state.progress.sweep, 1, "source churn is not a new sweep");
    }
    let revision = state.progress.active_revision;
    state = state
        .apply(M::PinPage {
            fence: 1,
            now: 11,
            page: page(revision, vec![], true),
        })
        .unwrap();
    state = state
        .apply(M::Advance {
            fence: 1,
            now: 11,
            active_revision: revision,
            page_sha256: "c".repeat(64),
        })
        .unwrap();
    assert!(state.progress.complete && state.settled_after.is_some());
    state = state
        .apply(M::Refresh {
            fence: 1,
            now: 11,
            active_revision: revision,
        })
        .unwrap();
    assert!(state.settled_after.is_none());
    assert_eq!(state.progress.sweep, 2);
}

#[test]
fn checkpoint_refuses_malformed_unbounded_and_intermediate_states() {
    assert!(serde_json::from_slice::<LedgerCollectionCheckpointV2>(
        &serde_json::to_vec(&initial().progress).unwrap()
    )
    .is_err());
    for case in 0..9 {
        let mut state = selected();
        match case {
            0 => state.progress.pending.as_mut().unwrap().phase = LedgerCollectionPhase::Prepared,
            1 => {
                state.progress.pending.as_mut().unwrap().phase =
                    LedgerCollectionPhase::SourceCommitted
            }
            2 => {
                state.accepted_artifact.as_mut().unwrap().artifact_lengths[2] = 4 * 1024 * 1024 + 1
            }
            3 => state.accepted_artifact = None,
            4 => state.acknowledged_revision = Some(8),
            5 => state.settled_after = Some("3".repeat(64)),
            6 => state.progress.page.as_mut().unwrap().next_cursor = Some("\n".into()),
            7 => {
                state = acknowledged();
                state.acknowledged_revision = Some(9);
            }
            _ => {
                state.progress.page.as_mut().unwrap().remaining_groups =
                    (0..65).map(|i| format!("{i:064x}")).collect()
            }
        }
        assert!(!state.validate(), "case {case}");
        assert!(state
            .apply(M::Claim {
                holder_sha256: "6".repeat(64),
                now: 20,
                expires_at: 30
            })
            .is_err());
    }
    let mut state = initial();
    state.progress.revision = u64::MAX;
    assert!(state
        .apply(M::PinPage {
            fence: 1,
            now: 11,
            page: page(7, vec![], true)
        })
        .is_err());
}
