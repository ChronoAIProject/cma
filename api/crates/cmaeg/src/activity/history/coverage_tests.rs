use super::{
    tests::{query, row, setup},
    *,
};
use std::cell::Cell;

#[tokio::test]
async fn coverage_reports_receipt_expiry_during_the_first_page() {
    let (memory, service, opened_ms) = setup();
    let oldest = opened_ms - service.config.retention_ms() + 1;
    memory.capture.lock().unwrap().since_ms = Some(oldest);
    service
        .received("owner", "channel", row("retained", opened_ms - 1))
        .await
        .unwrap();
    let query = HistoryQuery {
        from: Some(paging::format_time(oldest).unwrap()),
        until: Some(paging::format_time(opened_ms).unwrap()),
        ..Default::default()
    };
    let retained = service
        .page_with_clock("owner", "channel", query.clone(), || opened_ms)
        .await
        .unwrap();
    assert!(retained.coverage.complete_event_enumeration);

    // A TTL sweep can remove the oldest receipt before the snapshot observes
    // it. The inclusive lower bound reaches its expiry as this page completes.
    let clock = Cell::new(opened_ms);
    let page = service
        .page_with_clock("owner", "channel", query, || clock.replace(opened_ms + 1))
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].activity.event_id, "retained");
    assert!(!page.coverage.complete_event_enumeration);
    assert_eq!(page.coverage.gaps, vec![CoverageGap::RetentionExpired]);
    assert_eq!(
        page.coverage.retained_from,
        paging::format_time(oldest + 1).unwrap()
    );
}

#[tokio::test]
async fn longer_configuration_does_not_claim_expired_rows_or_extend_existing_receipts() {
    let (memory, mut service, at) = setup();
    service
        .received("owner", "channel", row("original", at))
        .await
        .unwrap();
    let first = service
        .page("owner", "channel", query(at, 100))
        .await
        .unwrap();
    assert!(first.coverage.complete_event_enumeration);
    service.config.retention_days = 14;
    assert!(matches!(
        service.received("owner", "channel", row("new", at)).await,
        Err(Error::InvalidConfiguration)
    ));
    let page = service
        .page("owner", "channel", query(at, 100))
        .await
        .unwrap();
    assert!(!page.coverage.complete_event_enumeration);
    assert!(page.coverage.gaps.contains(&CoverageGap::CaptureGap));
    assert_eq!(
        memory
            .events
            .lock()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .expires_ms,
        at + 7 * 86_400_000
    );
}

#[tokio::test]
async fn rollback_revision_invalidates_continuation_and_closed_interval_is_partial() {
    let (memory, mut service, at) = setup();
    for id in ["a", "b"] {
        service
            .received("owner", "channel", row(id, at))
            .await
            .unwrap();
    }
    let first = service
        .page("owner", "channel", query(at, 1))
        .await
        .unwrap();
    {
        let mut marker = memory.capture.lock().unwrap();
        marker.until_ms = Some(at);
        marker.revision += 1;
    }
    assert!(matches!(
        service
            .page(
                "owner",
                "channel",
                HistoryQuery {
                    cursor: first.next_cursor,
                    ..Default::default()
                }
            )
            .await,
        Err(Error::Expired)
    ));
    let page = service
        .page("owner", "channel", query(at, 100))
        .await
        .unwrap();
    assert!(page.coverage.gaps.contains(&CoverageGap::CaptureGap));
    memory.capture.lock().unwrap().until_ms = None;
    service.config.capture = false;
    assert!(
        !service
            .page("owner", "channel", query(at, 100))
            .await
            .unwrap()
            .coverage
            .complete_event_enumeration
    );
}

#[tokio::test]
async fn legacy_partial_status_persists_after_legacy_row_is_on_an_earlier_page() {
    let (_, service, at) = setup();
    service
        .import_legacy("owner", "channel", row("legacy", at + 1), at + 86_400_000)
        .await
        .unwrap();
    service
        .received("owner", "channel", row("live", at))
        .await
        .unwrap();
    let first = service
        .page("owner", "channel", query(at, 1))
        .await
        .unwrap();
    assert!(first.items[0].legacy_partial);
    let next = service
        .page(
            "owner",
            "channel",
            HistoryQuery {
                cursor: first.next_cursor,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(!next.items[0].legacy_partial);
    assert!(next.coverage.gaps.contains(&CoverageGap::LegacyPartial));
}
