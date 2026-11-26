use super::{tests::*, *};
use crate::activity::{Change, ChannelActivity, ConsumerState};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::sync::atomic::Ordering;

async fn large(service: &ActivityHistory, at: i64) {
    service
        .received("owner", "channel", row("large", at))
        .await
        .unwrap();
    for index in 0..16 {
        let mut change =
            Change::state(&format!("consumer-{index}"), ConsumerState::Completed, None);
        change.display_name = Some("detail".repeat(800));
        service
            .change("owner", "channel", "large", &change)
            .await
            .unwrap();
    }
}
#[tokio::test]
async fn oversized_summary_is_discoverable_and_details_pin_one_generation() {
    let (memory, service, at) = setup();
    large(&service, at).await;
    let first = service
        .page("owner", "channel", query(at, 100))
        .await
        .unwrap();
    let item = &first.items[0];
    assert!(item.activity.consumers.is_empty());
    assert!(!item.detail_incomplete);
    let token = item.detail_cursor.as_ref().unwrap();
    let before = service
        .details("owner", "channel", &item.history_event_id, token)
        .await
        .unwrap();
    let bytes: Vec<_> = before
        .parts
        .iter()
        .flat_map(|p| STANDARD.decode(&p.data).unwrap())
        .collect();
    let summary: ChannelActivity = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(summary.consumers.len(), 16);
    assert!(bytes.len() > 49_112);
    let mut change = Change::state("consumer-0", ConsumerState::Completed, None);
    change.display_name = Some("updated".into());
    service
        .change("owner", "channel", "large", &change)
        .await
        .unwrap();
    let pinned = service
        .details("owner", "channel", &item.history_event_id, token)
        .await
        .unwrap();
    assert_eq!(pinned.generation, before.generation);
    assert_eq!(pinned.parts[0].data, before.parts[0].data);
    assert!(matches!(
        service
            .details("other", "channel", &item.history_event_id, token)
            .await,
        Err(Error::InvalidRequest)
    ));
    assert!(matches!(
        service
            .details("owner", "other", &item.history_event_id, token)
            .await,
        Err(Error::InvalidRequest)
    ));
    assert!(matches!(
        service
            .details("owner", "channel", "other-event", token)
            .await,
        Err(Error::InvalidRequest)
    ));
    assert!(memory
        .chunks
        .lock()
        .unwrap()
        .values()
        .all(|c| c.expires_ms == at + 7 * 86_400_000));
}
#[tokio::test]
async fn failed_chunk_write_preserves_ingress_and_exposes_detail_gap() {
    let (memory, service, at) = setup();
    service
        .received("owner", "channel", row("large", at))
        .await
        .unwrap();
    memory.fail_chunks.store(true, Ordering::SeqCst);
    let mut change = Change::state("c", ConsumerState::Completed, None);
    change.display_name = Some("x".repeat(30_000));
    assert!(matches!(
        service.change("owner", "channel", "large", &change).await,
        Err(Error::Unavailable)
    ));
    let page = service
        .page("owner", "channel", query(at, 100))
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(page.items[0].detail_incomplete);
    assert_eq!(page.items[0].activity.event_id, "large");
}
#[tokio::test]
async fn byte_budget_yields_resumable_pages_without_dropping_events() {
    let (_, service, at) = setup();
    for index in 0..100 {
        let id = format!("event-{index}");
        service
            .received("owner", "channel", row(&id, at))
            .await
            .unwrap();
        let mut change = Change::state("c", ConsumerState::Completed, None);
        change.display_name = Some("x".repeat(22_000));
        service
            .change("owner", "channel", &id, &change)
            .await
            .unwrap();
    }
    let page = service
        .page("owner", "channel", query(at, 200))
        .await
        .unwrap();
    assert!(page.items.len() < 100 && page.has_more);
    assert!(serde_json::to_vec(&page).unwrap().len() <= 1_048_576);
    assert_eq!(all(&service, query(at, 200)).await.len(), 100);
}
#[tokio::test]
async fn capture_off_never_writes_and_failed_admission_is_not_success() {
    let (memory, mut service, at) = setup();
    service.config.capture = false;
    service
        .received("owner", "channel", row("off", at))
        .await
        .unwrap();
    assert!(memory.events.lock().unwrap().is_empty());
    service.config.capture = true;
    memory.fail_insert.store(true, Ordering::SeqCst);
    assert!(matches!(
        service.received("owner", "channel", row("fail", at)).await,
        Err(Error::Unavailable)
    ));
}
