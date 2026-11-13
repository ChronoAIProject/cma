use super::*;

fn initial() -> LedgerCollectorProgressV1 {
    LedgerCollectorProgressV1::new("a".repeat(64), 3, 7, "b".repeat(64), 10, 20).unwrap()
}

fn page(groups: Vec<String>, complete: bool) -> LedgerCollectionPageProgress {
    LedgerCollectionPageProgress {
        page_sha256: "c".repeat(64),
        active_revision: 7,
        cursor: None,
        next_cursor: (!complete).then(|| "source-key-64".into()),
        complete,
        remaining_groups: groups,
    }
}

fn pending(group: &str) -> LedgerCollectionPending {
    LedgerCollectionPending {
        operation_id: uuid::Uuid::new_v4().to_string(),
        group_key: group.repeat(64),
        request_sha256: "d".repeat(64),
        observation_sha256: "e".repeat(64),
        source_sha256: "f".repeat(64),
        phase: LedgerCollectionPhase::Selected,
        receipt_sha256: None,
    }
}

#[test]
fn collection_progress_restart_and_takeover_keep_original_operation() {
    let request = pending("1");
    let progress = initial()
        .pin_page(1, 11, page(vec!["1".repeat(64)], false))
        .unwrap()
        .select(1, 11, request.clone())
        .unwrap();
    let bytes = serde_json::to_vec(&progress).unwrap();
    let reopened: LedgerCollectorProgressV1 = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(progress, reopened);
    assert!(reopened.claim("2".repeat(64), 19, 30).is_err());
    let claimed = reopened.claim("2".repeat(64), 20, 30).unwrap();
    assert_eq!(claimed.lease.fence, 2);
    assert_eq!(claimed.pending, Some(request.clone()));
    assert_eq!(claimed.page, progress.page);
    assert_eq!(claimed.cursor, progress.cursor);
    assert!(claimed
        .record_outcome(
            1,
            21,
            &request.operation_id,
            LedgerCollectionPhase::Prepared,
            Some("3".repeat(64))
        )
        .is_err());
    assert!(claimed.rollover(2, 21, 8).is_err());
    assert!(claimed.advance(2, 21, 7, &"c".repeat(64)).is_err());
}

#[test]
fn collection_progress_requires_archive_source_and_ack_before_advance() {
    let request = pending("1");
    let mut progress = initial()
        .pin_page(1, 11, page(vec!["1".repeat(64)], false))
        .unwrap()
        .select(1, 11, request.clone())
        .unwrap();
    assert!(progress
        .record_outcome(
            1,
            11,
            &request.operation_id,
            LedgerCollectionPhase::Acknowledged,
            Some("4".repeat(64))
        )
        .is_err());
    for phase in [
        LedgerCollectionPhase::Prepared,
        LedgerCollectionPhase::SourceCommitted,
        LedgerCollectionPhase::Acknowledged,
    ] {
        assert!(progress.advance(1, 12, 7, &"c".repeat(64)).is_err());
        progress = progress
            .record_outcome(1, 12, &request.operation_id, phase, Some("4".repeat(64)))
            .unwrap();
        assert_eq!(
            progress
                .record_outcome(1, 12, &request.operation_id, phase, Some("4".repeat(64)))
                .unwrap(),
            progress,
            "same outcome replay is stable"
        );
    }
    assert!(progress.settle_group(1, 12, "different-operation").is_err());
    progress = progress.settle_group(1, 12, &request.operation_id).unwrap();
    assert_eq!(progress.last_ack_sha256, Some("4".repeat(64)));
    assert!(progress.advance(1, 12, 8, &"c".repeat(64)).is_err());
    assert!(progress.advance(1, 12, 7, &"5".repeat(64)).is_err());
    let advanced = progress.advance(1, 12, 7, &"c".repeat(64)).unwrap();
    assert_eq!(advanced.cursor.as_deref(), Some("source-key-64"));
    assert!(!advanced.complete);
    assert!(advanced.pending.is_none() && advanced.page.is_none());
}

#[test]
fn collection_progress_cannot_drop_other_groups_at_a_page_boundary() {
    let mut progress = initial()
        .pin_page(1, 11, page(vec!["1".repeat(64), "2".repeat(64)], true))
        .unwrap();
    for key in ["1", "2"] {
        let request = pending(key);
        progress = progress
            .select(1, 12, request.clone())
            .unwrap()
            .record_outcome(
                1,
                12,
                &request.operation_id,
                LedgerCollectionPhase::Retained,
                None,
            )
            .unwrap()
            .settle_group(1, 12, &request.operation_id)
            .unwrap();
        if key == "1" {
            assert!(progress.advance(1, 12, 7, &"c".repeat(64)).is_err());
        }
    }
    let complete = progress.advance(1, 12, 7, &"c".repeat(64)).unwrap();
    assert!(complete.complete);
    assert!(complete.select(1, 12, pending("3")).is_err());
    let next = complete.rollover(1, 12, 8).unwrap();
    assert!(!next.complete);
    assert_eq!(next.sweep, 2);
    assert_eq!(next.active_revision, 8);
}

#[test]
fn collection_progress_empty_page_is_not_implicit_completion() {
    let initial = initial();
    assert!(initial.advance(1, 11, 7, &"c".repeat(64)).is_err());
    let more = initial
        .pin_page(1, 11, page(vec![], false))
        .unwrap()
        .advance(1, 11, 7, &"c".repeat(64))
        .unwrap();
    assert!(!more.complete);
    let end = initial
        .pin_page(1, 11, page(vec![], true))
        .unwrap()
        .advance(1, 11, 7, &"c".repeat(64))
        .unwrap();
    assert!(end.complete);
    for case in 0..5 {
        let mut invalid = page(vec![], false);
        match case {
            0 => invalid.active_revision = 8,
            1 => invalid.cursor = Some("different-source-key".into()),
            2 => invalid.remaining_groups = vec!["1".repeat(64); 65],
            3 => invalid.next_cursor = None,
            _ => invalid.remaining_groups = vec!["2".repeat(64), "1".repeat(64)],
        }
        assert!(initial.pin_page(1, 11, invalid).is_err(), "case {case}");
    }
}

#[test]
fn collection_progress_cancellation_and_overflow_never_clear_pending() {
    let mut value = initial();
    value.revision = u64::MAX;
    assert!(value.claim("2".repeat(64), 20, 30).is_err());
    value = initial();
    value.lease.fence = u64::MAX;
    assert!(value.claim("2".repeat(64), 20, 30).is_err());
    let mut json = serde_json::to_value(initial()).unwrap();
    json["authority_override"] = true.into();
    assert!(serde_json::from_value::<LedgerCollectorProgressV1>(json).is_err());
    let value = initial()
        .pin_page(1, 11, page(vec!["1".repeat(64)], false))
        .unwrap();
    assert!(value.select(1, 20, pending("1")).is_err());
    assert!(value.select(1, 12, pending("2")).is_err());
    assert_eq!(value.page.unwrap().remaining_groups, ["1".repeat(64)]);
}

#[test]
fn collection_progress_rollover_requires_a_new_revision_or_completed_sweep() {
    let value = initial()
        .pin_page(1, 11, page(vec!["1".repeat(64)], false))
        .unwrap();
    assert_eq!(
        value.rollover(1, 12, 7),
        Err(LedgerCollectionProgressError::StaleCursor)
    );
    assert_eq!(
        value.rollover(1, 12, 6),
        Err(LedgerCollectionProgressError::StaleCursor)
    );
    let fresh = value.rollover(1, 12, 8).unwrap();
    assert_eq!(fresh.sweep, 2);
    assert!(fresh.page.is_none() && fresh.cursor.is_none() && !fresh.complete);
    assert!(initial().pin_page(1, -1, page(vec![], true)).is_err());
    assert!(LedgerCollectorProgressV1::new("a".repeat(64), 3, 7, "b".repeat(64), -2, -1,).is_err());
}
