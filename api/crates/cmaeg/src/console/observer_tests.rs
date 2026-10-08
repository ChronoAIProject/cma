use std::time::{Duration, Instant};

use cmaeg_core::{
    activity::{ActivityConversation, Actor, ActorKind, ConversationKind},
    channel::Platform,
};

use super::*;

pub(crate) fn record() -> RecordId {
    RecordId::new("0b6f3c4e-8a59-4f3b-9d55-2f1c8f2e7a10").unwrap()
}

/// A relayed Telegram message carrying text, an actor, a chat and a caption
/// that must never reach the console.
pub(crate) fn activity(event_id: &str) -> Activity {
    Activity {
        schema_version: 1,
        event_id: EventId::new(event_id).unwrap(),
        kind: ActivityKind::Message {
            text: "fixture message text".into(),
            attachments: Vec::new(),
            mentions_bot: true,
            reply_to: None,
            album_id: None,
        },
        actor: Actor {
            id: "123456789".into(),
            display_name: Some("Fixture Person".into()),
            kind: ActorKind::Human,
        },
        conversation: ActivityConversation {
            id: "-1001234567890".into(),
            kind: ConversationKind::Group,
            thread_id: None,
        },
        occurred_at: "2026-10-08T00:00:00Z".into(),
        source: SourceKind::NyxidRelay {
            platform: Platform::Telegram,
            route_id: "route-1".into(),
        },
    }
}

#[test]
fn a_full_queue_drops_and_counts_without_blocking() {
    let (observer, mut queue) = ConsoleObserver::new();
    let started = Instant::now();
    for index in 0..=QUEUE_CAPACITY {
        observer.changed(&record(), "alice", index as u64);
    }
    // try_send never waits: 4,097 calls return at once while nothing drains.
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(observer.dropped(), 1);
    let mut queued = 0;
    while let Some(observation) = queue.try_next() {
        assert!(matches!(observation, Observation::IndexChanged { .. }));
        queued += 1;
    }
    assert_eq!(queued, QUEUE_CAPACITY);
    // Room again: the next observation is queued.
    observer.removed(&record());
    assert_eq!(observer.dropped(), 1);
    assert!(matches!(
        queue.try_next(),
        Some(Observation::IndexRemoved { .. })
    ));
}

#[test]
fn a_closed_queue_drops_and_counts() {
    let (observer, queue) = ConsoleObserver::new();
    drop(queue);
    observer.removed(&record());
    assert_eq!(observer.dropped(), 1);
}

#[test]
fn admitted_events_are_masked_before_they_are_queued() {
    let (observer, mut queue) = ConsoleObserver::new();
    observer.admitted(&record(), &activity("telegram:777:42"));
    let Some(Observation::Routed {
        record: kept,
        entry_seed,
    }) = queue.try_next()
    else {
        panic!("expected a routed observation");
    };
    assert_eq!(kept, record());
    assert_eq!(entry_seed.event_key, mask::event_key("telegram:777:42"));
    assert_eq!(entry_seed.actor_masked, "…6789");
    assert_eq!(entry_seed.chat_masked, "…7890");
    assert_eq!(entry_seed.source, "nyxid_relay:telegram");
    assert_eq!(entry_seed.kind, "message");
    let dump = serde_json::to_string(&Observation::Routed {
        record: kept,
        entry_seed,
    })
    .unwrap();
    for raw in [
        "fixture message text",
        "Fixture Person",
        "123456789",
        "-1001234567890",
        "telegram:777:42",
    ] {
        assert!(!dump.contains(raw), "{raw} leaked into {dump}");
    }
}

#[test]
fn refusals_launches_and_deliveries_keep_only_bounded_labels() {
    let (observer, mut queue) = ConsoleObserver::new();
    let event = EventId::new("evt-1").unwrap();
    let consumer = ConsumerId::default_consumer();
    observer.refused(&record(), "custom", Some(&event), "Bearer abc rejected!");
    observer.launched(
        &record(),
        &event,
        &consumer,
        LaunchClass::Accepted,
        Duration::from_millis(12),
    );
    observer.launched(
        &record(),
        &event,
        &consumer,
        LaunchClass::Refused,
        Duration::from_millis(5),
    );
    observer.delivered(
        &record(),
        &event,
        &consumer,
        DeliveryStep::Answer,
        &DeliveryOutcome::Rejected {
            reason: "chat not found: -100123".into(),
        },
        Destination::Origin,
    );
    let Some(Observation::Refused { seed, .. }) = queue.try_next() else {
        panic!("expected a refusal");
    };
    assert_eq!(seed.reason, "other");
    assert_eq!(
        seed.event_key.as_deref(),
        Some(mask::event_key("evt-1").as_str())
    );
    assert!(matches!(
        queue.try_next(),
        Some(Observation::Launch {
            outcome: LaunchClass::Accepted,
            accepted_ms: Some(12),
            ..
        })
    ));
    assert!(matches!(
        queue.try_next(),
        Some(Observation::Launch {
            outcome: LaunchClass::Refused,
            accepted_ms: None,
            ..
        })
    ));
    let Some(Observation::Delivery { outcome, .. }) = queue.try_next() else {
        panic!("expected a delivery");
    };
    assert_eq!(
        outcome,
        DeliveryClass::Rejected {
            reason: "other".into()
        }
    );
}

#[test]
fn producers_are_stripped_and_bounded() {
    let (observer, mut queue) = ConsoleObserver::new();
    observer.produced(
        &record(),
        ProducerFacts {
            source: "https://user:pw@sensors.example/a?token=x",
            hint: "hmac:primary",
            ce_type: &"t".repeat(300),
            outcome: LaunchClass::Accepted,
        },
    );
    let Some(Observation::Producer {
        source_uri,
        hint,
        ce_type,
        ..
    }) = queue.try_next()
    else {
        panic!("expected a producer");
    };
    assert_eq!(source_uri, "https://sensors.example/a");
    assert_eq!(hint, "hmac:primary");
    assert_eq!(ce_type.len(), CE_TYPE_BYTES);
}

#[tokio::test]
async fn the_drainer_applies_observations_in_order() {
    let (observer, queue) = ConsoleObserver::new();
    observer.changed(&record(), "alice", 1);
    observer.observed(&record(), &ChannelId::new("cmaeg1.ch.fixture").unwrap());
    observer.removed(&record());
    drop(observer);
    let mut seen = Vec::new();
    queue
        .drain(|observation| {
            seen.push(match observation {
                Observation::IndexChanged { .. } => "changed",
                Observation::ChannelId { .. } => "observed",
                Observation::IndexRemoved { .. } => "removed",
                _ => "other",
            })
        })
        .await;
    assert_eq!(seen, ["changed", "observed", "removed"]);
}
