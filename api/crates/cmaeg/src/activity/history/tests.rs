use super::*;
use crate::{
    activity::{Change, ChannelActivity, ConsumerState},
    ingress::routing_plan::fixture,
};
use std::sync::Arc;
pub(super) fn setup() -> (Arc<test_repository::Memory>, ActivityHistory, i64) {
    let memory = Arc::new(test_repository::Memory::default());
    let now = now_ms() - 10_000;
    memory.capture.lock().unwrap().since_ms = Some(now - 1000);
    let service = ActivityHistory::new(
        Some(memory.clone()),
        Arc::new(fixture::keys()),
        Config {
            enabled: true,
            capture: true,
            ..Default::default()
        },
    );
    (memory, service, now)
}
pub(super) fn row(id: &str, at: i64) -> ChannelActivity {
    let activity = fixture::message("private-content-must-not-be-retained");
    ChannelActivity {
        event_id: id.into(),
        quoted_context: None,
        kind: Some("message".into()),
        event_type: None,
        source_occurred_at: None,
        source: activity.source,
        actor: activity.actor,
        conversation: activity.conversation,
        occurred_at: activity.occurred_at,
        received_at: paging::format_time(at).unwrap(),
        consumers: vec![],
    }
}
pub(super) fn query(at: i64, limit: u16) -> HistoryQuery {
    HistoryQuery {
        from: Some(paging::format_time(at - 1000).unwrap()),
        until: Some(paging::format_time(at + 1000).unwrap()),
        limit: Some(limit),
        cursor: None,
    }
}
pub(super) async fn all(service: &ActivityHistory, mut query: HistoryQuery) -> Vec<HistoryItem> {
    let mut items = vec![];
    loop {
        let page = service
            .page("owner", "channel", query.clone())
            .await
            .unwrap();
        assert_eq!(page.has_more, page.next_cursor.is_some());
        items.extend(page.items);
        if let Some(cursor) = page.next_cursor {
            query = HistoryQuery {
                cursor: Some(cursor),
                ..Default::default()
            };
        } else {
            break;
        }
    }
    items
}
#[tokio::test]
async fn all_55_receipts_survive_restart_and_daily_byte_budget() {
    let (memory, service, at) = setup();
    for index in 0..55 {
        service
            .received("owner", "channel", row(&format!("event-{index:03}"), at))
            .await
            .unwrap();
        let mut change = Change::state("consumer", ConsumerState::Completed, None);
        change.display_name = Some("x".repeat(1000));
        service
            .change("owner", "channel", &format!("event-{index:03}"), &change)
            .await
            .unwrap();
    }
    assert!(
        memory
            .events
            .lock()
            .unwrap()
            .values()
            .map(|r| r.blob.len())
            .sum::<usize>()
            > 44_000
    );
    let restarted = ActivityHistory::new(
        Some(memory.clone()),
        Arc::new(fixture::keys()),
        service.config.clone(),
    );
    let items = all(&restarted, query(at, 11)).await;
    let ids: std::collections::BTreeSet<_> =
        items.iter().map(|r| r.activity.event_id.as_str()).collect();
    assert_eq!(ids.len(), 55);
    assert!(ids.contains("event-000") && ids.contains("event-054"));
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
    assert!(!serde_json::to_string(&items)
        .unwrap()
        .contains("private-content-must-not-be-retained"));
}
#[tokio::test]
async fn membership_is_frozen_for_late_inserts_and_ties_across_replicas() {
    let (memory, service, at) = setup();
    for id in ["a", "b", "c"] {
        service
            .received("owner", "channel", row(id, at))
            .await
            .unwrap();
    }
    let first = service
        .page("owner", "channel", query(at, 1))
        .await
        .unwrap();
    let mut ids = vec![first.items[0].activity.event_id.clone()];
    service
        .received("owner", "channel", row("late", at - 500))
        .await
        .unwrap();
    let restarted =
        ActivityHistory::new(Some(memory), service.keys.clone(), service.config.clone());
    let mut cursor = first.next_cursor;
    while let Some(token) = cursor {
        let page = restarted
            .page(
                "owner",
                "channel",
                HistoryQuery {
                    cursor: Some(token),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        ids.extend(page.items.into_iter().map(|r| r.activity.event_id));
        cursor = page.next_cursor;
    }
    ids.sort();
    assert_eq!(ids, vec!["a", "b", "c"]);
}
#[tokio::test]
async fn duplicate_across_midnight_preserves_identity_expiry_and_first_time() {
    let (memory, service, at) = setup();
    let yesterday = at - 86_400_000;
    service
        .received("owner", "channel", row("one", yesterday))
        .await
        .unwrap();
    service
        .received("owner", "channel", row("one", at))
        .await
        .unwrap();
    let rows = memory.events.lock().unwrap();
    assert_eq!(rows.len(), 1);
    let stored = rows.values().next().unwrap();
    assert_eq!(stored.key.received_ms, yesterday);
    assert_eq!(stored.expires_ms, yesterday + 7 * 86_400_000);
}
#[tokio::test]
async fn cursor_tampering_owner_channel_query_purpose_and_expiry_are_rejected() {
    let (_, service, at) = setup();
    for id in ["a", "b"] {
        service
            .received("owner", "channel", row(id, at))
            .await
            .unwrap();
    }
    let token = service
        .page("owner", "channel", query(at, 1))
        .await
        .unwrap()
        .next_cursor
        .unwrap();
    let q = HistoryQuery {
        cursor: Some(token.clone()),
        ..Default::default()
    };
    assert!(matches!(
        service.page("other", "channel", q.clone()).await,
        Err(Error::InvalidRequest)
    ));
    assert!(matches!(
        service.page("owner", "other", q.clone()).await,
        Err(Error::InvalidRequest)
    ));
    assert!(matches!(
        service
            .page(
                "owner",
                "channel",
                HistoryQuery {
                    limit: Some(2),
                    ..q.clone()
                }
            )
            .await,
        Err(Error::InvalidRequest)
    ));
    assert!(matches!(
        service
            .page(
                "owner",
                "channel",
                HistoryQuery {
                    cursor: Some(format!("{token}x")),
                    ..Default::default()
                }
            )
            .await,
        Err(Error::InvalidRequest)
    ));
    assert!(matches!(
        service.details("owner", "channel", "a", &token).await,
        Err(Error::InvalidRequest)
    ));
    let mut c: cursor::PageCursor = cursor::open(&service.keys, cursor::HISTORY, &token).unwrap();
    c.deadline_ms = now_ms() - 1;
    let expired = cursor::seal(&service.keys, cursor::HISTORY, &c).unwrap();
    assert!(matches!(
        service
            .page(
                "owner",
                "channel",
                HistoryQuery {
                    cursor: Some(expired),
                    ..Default::default()
                }
            )
            .await,
        Err(Error::Expired)
    ));
}
#[tokio::test]
async fn exact_range_empty_page_limits_retention_and_unsupported_backend() {
    let (_, service, at) = setup();
    for (id, t) in [("from", at - 1000), ("until", at + 1000)] {
        service
            .received("owner", "channel", row(id, t))
            .await
            .unwrap();
    }
    let page = service
        .page("owner", "channel", query(at, 200))
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].activity.event_id, "from");
    assert!(!page.has_more);
    for limit in [0, 201] {
        assert!(matches!(
            service.page("owner", "channel", query(at, limit)).await,
            Err(Error::InvalidRequest)
        ));
    }
    let mut old = query(at - 7 * 86_400_000, 100);
    old.until = Some(paging::format_time(at - 6 * 86_400_000).unwrap());
    let page = service.page("owner", "channel", old).await.unwrap();
    assert!(page.items.is_empty());
    assert!(page.coverage.gaps.contains(&CoverageGap::RetentionExpired));
    assert!(!page.coverage.complete_event_enumeration);
    let remote = ActivityHistory::new(None, service.keys.clone(), service.config.clone());
    assert!(matches!(
        remote.page("owner", "channel", query(at, 10)).await,
        Err(Error::UnsupportedBackend)
    ));
}
#[tokio::test]
async fn corrupt_neighbor_advances_cursor_and_keeps_gap_on_following_pages() {
    let (memory, service, at) = setup();
    for (id, t) in [("bad", at + 1), ("good", at)] {
        service
            .received("owner", "channel", row(id, t))
            .await
            .unwrap();
    }
    memory
        .events
        .lock()
        .unwrap()
        .get_mut(&event_key(&scope("owner", "channel"), "bad"))
        .unwrap()
        .blob = b"bad seal".to_vec();
    let page = service
        .page("owner", "channel", query(at, 1))
        .await
        .unwrap();
    assert!(page.items.is_empty());
    assert!(page.has_more);
    assert!(page.coverage.gaps.contains(&CoverageGap::UnreadableRecord));
    let next = service
        .page(
            "owner",
            "channel",
            HistoryQuery {
                cursor: page.next_cursor,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(next.items[0].activity.event_id, "good");
    assert!(next.coverage.gaps.contains(&CoverageGap::UnreadableRecord));
}
