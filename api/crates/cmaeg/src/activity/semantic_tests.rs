use super::*;
use crate::ingress::{routing_plan::fixture, routing_store::test_server::Server};
use cmaeg_core::activity::{ActivityKind, ActorKind};
use serde_json::json;

#[tokio::test]
async fn message_event_receipts_keep_type_and_reported_time_without_copying_payloads() {
    let server = Server::start_realtime().await;
    let keys = Arc::new(fixture::keys());
    let store = ActivityStore::new(server.records.clone(), keys.clone());
    let mut updates = store.subscribe();
    let mut activity = fixture::message("private-message-body");
    activity.event_id = "deleted".parse().unwrap();
    activity.actor.id = "unknown".into();
    activity.actor.kind = ActorKind::Unknown;
    activity.kind = ActivityKind::Event {
        ce: serde_json::from_value(json!({
            "specversion":"1.0", "id":"deleted", "source":"urn:discord:bot:77",
            "type":"cma.message.deleted.v1", "subject":"message-1",
            "data":{"original_text":"private-message-body", "original_event_id":"original"}
        }))
        .unwrap(),
        attachments: Vec::new(),
    };
    activity.validate().unwrap();
    let now = time::OffsetDateTime::now_utc();
    store
        .received_at("owner", "record", &activity, now)
        .await
        .unwrap();
    updates.try_recv().unwrap();
    // Restore from encrypted durable storage, independently of process-local state.
    let restored = ActivityStore::new(server.records.clone(), keys);
    let page = restored.latest("owner", "record").await.unwrap();
    let row = &page.items[0];
    assert_eq!(row.kind.as_deref(), Some("event"));
    assert_eq!(row.event_type.as_deref(), Some("cma.message.deleted.v1"));
    assert!(row.source_occurred_at.is_none());
    assert_eq!(row.actor.kind, ActorKind::Unknown);
    assert!(!serde_json::to_string(&page)
        .unwrap()
        .contains("private-message-body"));
    let first_receipt = row.received_at.clone();

    activity.event_id = "edited".parse().unwrap();
    let ActivityKind::Event { ce, .. } = &mut activity.kind else {
        unreachable!()
    };
    ce.id = "edited".into();
    ce.r#type = "cma.message.updated.v1".into();
    ce.time = Some("2026-10-27T08:00:00Z".into());
    store
        .received_at("owner", "record", &activity, now)
        .await
        .unwrap();
    let page = restored.latest("owner", "record").await.unwrap();
    let edited = page
        .items
        .iter()
        .find(|row| row.event_id == "edited")
        .unwrap();
    assert_eq!(
        edited.source_occurred_at.as_deref(),
        Some("2026-10-27T08:00:00Z")
    );
    assert_eq!(edited.event_type.as_deref(), Some("cma.message.updated.v1"));
    assert_eq!(
        page.items
            .iter()
            .find(|row| row.event_id == "deleted")
            .unwrap()
            .received_at,
        first_receipt
    );
}

#[test]
fn historical_activity_stays_unknown_without_inventing_type_or_source_time() {
    let activity = fixture::message("private-message-body");
    let value = json!({
        "event_id":"historical", "source":activity.source, "actor":activity.actor,
        "conversation":activity.conversation, "occurred_at":activity.occurred_at,
        "received_at":"2026-10-27T08:00:00Z", "consumers":[]
    });
    let row: ChannelActivity = serde_json::from_value(value.clone()).unwrap();
    assert!(row.kind.is_none());
    assert!(row.event_type.is_none());
    assert!(row.source_occurred_at.is_none());
    assert_eq!(serde_json::to_value(row).unwrap(), value);
}

#[test]
fn confirmed_receipts_and_local_buffering_remain_distinct_and_uncertainty_can_recover() {
    use crate::{
        activity::facts,
        delivery::intents::DeliveryOutcome,
        turns::observer::{DeliveryStep, Destination},
    };
    let request = fixture::request(|_| {}, fixture::message("fixture"));
    let mut row = ChannelActivity {
        quoted_context: None,
        event_id: "event-1".into(),
        kind: None,
        event_type: None,
        source_occurred_at: None,
        source: request.activity.source.clone(),
        actor: request.activity.actor.clone(),
        conversation: request.activity.conversation.clone(),
        occurred_at: request.activity.occurred_at,
        received_at: crate::activity::now(),
        consumers: vec![],
    };
    Change::state("default", ConsumerState::Unknown, None).apply(&mut row);
    Change::state("default", ConsumerState::Produced, Some("response".into())).apply(&mut row);
    assert_eq!(row.consumers[0].state, ConsumerState::Produced);
    let confirmed = DeliveryOutcome::Confirmed {
        platform_message_id: None,
    };
    let change = facts::delivery(
        "default",
        DeliveryStep::Answer,
        &confirmed,
        Destination::Origin,
        &cmaeg_core::seal::ReplyAddress::Sync {
            waiter: "local".into(),
        },
        None,
    );
    let delivery = change.delivery.unwrap();
    assert_eq!(delivery.outcome, "buffered");
    assert!(delivery.confirmed_at.is_none());
    let change = facts::delivery(
        "default",
        DeliveryStep::Answer,
        &confirmed,
        Destination::Origin,
        &cmaeg_core::seal::ReplyAddress::ResultWebhook {
            url: "https://user:secret@hook.example/private-token?access=credential".into(),
            hmac_key_id: "fixture".into(),
        },
        None,
    );
    let delivery = change.delivery.unwrap();
    assert_eq!(delivery.outcome, "confirmed");
    assert!(delivery.confirmed_at.is_some());
    assert_eq!(delivery.recipients[0].id, "https://hook.example");
}
