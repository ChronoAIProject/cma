use super::{
    tests::{query, row, setup},
    *,
};

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
