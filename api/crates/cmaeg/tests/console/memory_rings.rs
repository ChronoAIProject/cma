//! Activity rings and producer registries: byte accounting, per-channel
//! and global eviction, masking at storage, and that only authenticated
//! events ever enter them.
use std::sync::Arc;

use cmaeg::{
    console::{
        activity::{ActivityFilter, ActivityRings, Update},
        mask,
        memory::ConsoleMemory,
        observer::{ActivitySeed, ConsoleObserver, Observation, RefusalSeed},
        producers::Seen,
        summary::ChannelSummary,
    },
    http::readiness::Readiness,
    ingress::IngressState,
    turns::observer::{
        ActivityObserver, DeliveryStep, Destination, LaunchClass, ObservedLauncher, ProducerFacts,
    },
    ConsoleMemoryConfig,
};
use cmaeg_core::{
    activity::{
        Activity, ActivityConversation, ActivityKind, Actor, ActorKind, Attachment,
        ConversationKind, SourceKind,
    },
    channel::{ChannelDefinition, Secret},
    cloudevent::CloudEvent,
    ids::{ConsumerId, EventId, RecordId},
};
use serde_json::json;
use time::OffsetDateTime;

use super::{schedule, NoRecords};
use crate::observations::{cloud_event, custom, send_custom};

fn record(index: usize) -> RecordId {
    RecordId::new(format!("00000000-0000-4000-8000-{index:012}")).unwrap()
}

fn seed(event: &str) -> ActivitySeed {
    ActivitySeed {
        at: OffsetDateTime::now_utc(),
        event_key: mask::event_key(event),
        source: "custom".into(),
        kind: "event",
        actor_masked: "…1234".into(),
        chat_masked: "…5678".into(),
        attachments: [0; 4],
        routing: None,
        trace: None,
        refusal: None,
        causation: None,
    }
}

fn weight_of(rings: &ActivityRings, record: &RecordId) -> usize {
    rings
        .entries(record, &ActivityFilter::default(), usize::MAX)
        .iter()
        .map(|entry| entry.weight())
        .sum::<usize>()
        + rings
            .producers(record)
            .iter()
            .map(|producer| producer.weight())
            .sum::<usize>()
}

#[test]
fn entries_count_their_bytes_as_they_grow() {
    let rings = ActivityRings::new(100, 1 << 20, 32);
    for index in 0..3 {
        rings.admitted(record(1), seed(&format!("evt-{index}")));
    }
    let before = rings.totals();
    assert_eq!(before.entries, 3);
    assert_eq!(before.bytes, weight_of(&rings, &record(1)));
    rings.update(
        &record(1),
        &mask::event_key("evt-1"),
        Update::Launch {
            consumer: ConsumerId::default_consumer(),
            outcome: LaunchClass::Accepted,
            accepted_ms: Some(40),
        },
    );
    rings.update(
        &record(1),
        &mask::event_key("evt-1"),
        Update::Delivery {
            consumer: ConsumerId::default_consumer(),
            step: DeliveryStep::ToolReply,
            outcome: cmaeg::console::observer::DeliveryClass::Confirmed,
            destination: Destination::Origin,
        },
    );
    let after = rings.totals();
    assert!(after.bytes > before.bytes);
    assert_eq!(after.bytes, weight_of(&rings, &record(1)));
    let entry = rings.event(&record(1), &mask::event_key("evt-1")).unwrap();
    assert_eq!(entry.consumers.len(), 1);
    assert_eq!(entry.consumers[0].launch, Some(LaunchClass::Accepted));
    assert!(entry.consumers[0].tool_replied);
    // An update for an event no ring holds changes nothing.
    rings.update(
        &record(1),
        &mask::event_key("evt-unknown"),
        Update::Launch {
            consumer: ConsumerId::default_consumer(),
            outcome: LaunchClass::Refused,
            accepted_ms: None,
        },
    );
    assert_eq!(rings.totals(), after);
}

#[test]
fn a_channel_keeps_its_newest_entries() {
    let rings = ActivityRings::new(10, 1 << 20, 32);
    for index in 0..15 {
        rings.admitted(record(1), seed(&format!("evt-{index}")));
    }
    let entries = rings.entries(&record(1), &ActivityFilter::default(), usize::MAX);
    assert_eq!(entries.len(), 10);
    assert_eq!(entries[0].event_key, Some(mask::event_key("evt-14")));
    assert_eq!(entries[9].event_key, Some(mask::event_key("evt-5")));
    assert_eq!(rings.totals().entries, 10);
    assert_eq!(rings.totals().bytes, weight_of(&rings, &record(1)));
}

#[test]
fn a_hot_channel_evicts_the_oldest_entries_globally() {
    let probe = ActivityRings::new(100, 1 << 20, 32);
    probe.admitted(record(9), seed("evt-probe"));
    let one = probe.totals().bytes;
    let rings = ActivityRings::new(100, one * 10, 32);
    for index in 0..5 {
        rings.admitted(record(1), seed(&format!("quiet-{index}")));
    }
    for index in 0..20 {
        rings.admitted(record(2), seed(&format!("hot-{index:02}")));
    }
    let totals = rings.totals();
    assert!(totals.bytes <= one * 10, "{totals:?}");
    assert!(rings
        .entries(&record(1), &ActivityFilter::default(), usize::MAX)
        .is_empty());
    let hot = rings.entries(&record(2), &ActivityFilter::default(), usize::MAX);
    assert_eq!(hot.len(), 10);
    assert_eq!(hot[0].event_key, Some(mask::event_key("hot-19")));
    assert_eq!(totals.channels, 1);
}

#[test]
fn refusals_are_entries_filterable_as_refused() {
    let rings = ActivityRings::new(100, 1 << 20, 32);
    rings.admitted(record(1), seed("evt-1"));
    rings.refused(
        record(1),
        RefusalSeed {
            at: OffsetDateTime::now_utc(),
            event_key: None,
            source: "nyxid_relay",
            reason: "sender_denied".into(),
        },
    );
    let refused = ActivityFilter {
        outcome: Some(LaunchClass::Refused),
        ..ActivityFilter::default()
    };
    let entries = rings.entries(&record(1), &refused, usize::MAX);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].refusal.as_deref(), Some("sender_denied"));
    let relay = ActivityFilter {
        source: Some("custom".into()),
        ..ActivityFilter::default()
    };
    assert_eq!(rings.entries(&record(1), &relay, usize::MAX).len(), 1);
}

fn seen(source: &str) -> Seen {
    Seen {
        source_uri: mask::source_uri(source),
        hint: "hmac:ingress".into(),
        ce_type: "order.created".into(),
        outcome: LaunchClass::Accepted,
        at: OffsetDateTime::now_utc(),
    }
}

#[test]
fn producers_are_stripped_bounded_and_counted() {
    let rings = ActivityRings::new(100, 1 << 20, 2);
    rings.produced(record(1), seen("https://user:pw@a.example/x?token=1#f"));
    rings.produced(record(1), seen("https://a.example/x"));
    rings.produced(record(1), seen("https://b.example/y"));
    rings.produced(record(1), seen("https://c.example/z"));
    let producers = rings.producers(&record(1));
    let sources: Vec<_> = producers.iter().map(|p| p.source_uri.as_str()).collect();
    assert_eq!(sources, ["https://c.example/z", "https://b.example/y"]);
    assert_eq!(rings.totals().bytes, weight_of(&rings, &record(1)));
    rings.produced(record(1), seen("https://b.example/y?again"));
    let producers = rings.producers(&record(1));
    assert_eq!(producers[0].source_uri, "https://b.example/y");
    assert_eq!(producers[0].count, 2);
    assert_eq!(rings.totals().bytes, weight_of(&rings, &record(1)));
}

#[test]
fn producers_alone_never_outgrow_the_budget() {
    let probe = ActivityRings::new(100, 1 << 20, 32);
    probe.produced(record(9), seen("https://a.example/0"));
    let one = probe.totals().bytes;
    let rings = ActivityRings::new(100, one * 3, 32);
    for index in 0..10 {
        rings.produced(record(index), seen(&format!("https://a.example/{index}")));
    }
    assert!(rings.totals().bytes <= one * 3);
    assert_eq!(rings.totals().channels, 3);
}

/// Everything a fixture event carries that the console must never keep.
const LEAKS: [&str; 8] = [
    "fixture message text",
    "fixture-cloud-data",
    "telegram:fixture-file",
    "https://files.example/fixture.png",
    "fixture-reply-token",
    "fixture-channel-key",
    "fixture-hmac-secret",
    "Fixture Person",
];

fn activities() -> Vec<Activity> {
    let attachment: Attachment = serde_json::from_value(json!({
        "reference": "telegram:fixture-file",
        "kind": "image",
        "filename": "https://files.example/fixture.png"
    }))
    .unwrap();
    let actor = Actor {
        id: "123456789".into(),
        display_name: Some("Fixture Person".into()),
        kind: ActorKind::Human,
    };
    let conversation = ActivityConversation {
        id: "-1001234567890".into(),
        kind: ConversationKind::Group,
        thread_id: None,
    };
    let ce: CloudEvent = serde_json::from_value(json!({
        "specversion":"1.0","id":"evt-ce","type":"order.created",
        "source":"https://user:fixture-reply-token@orders.example/a?key=fixture-hmac-secret",
        "data":{"note":"fixture-cloud-data"}
    }))
    .unwrap();
    vec![
        Activity {
            thread: None,
            schema_version: 1,
            event_id: EventId::new("evt-message").unwrap(),
            kind: ActivityKind::Message {
                text: "fixture message text".into(),
                attachments: vec![attachment.clone()],
                mentions_bot: true,
                reply_to: None,
                quoted_message: None,
                album_id: None,
            },
            actor: actor.clone(),
            conversation: conversation.clone(),
            occurred_at: "2026-10-08T00:00:00Z".into(),
            source: SourceKind::Custom,
            routing: None,
        },
        Activity {
            thread: None,
            schema_version: 1,
            event_id: EventId::new("evt-ce").unwrap(),
            kind: ActivityKind::Event {
                ce,
                attachments: vec![attachment],
            },
            actor,
            conversation,
            occurred_at: "2026-10-08T00:00:00Z".into(),
            source: SourceKind::Custom,
            routing: None,
        },
    ]
}

#[tokio::test]
async fn no_structure_holds_content_references_tokens_or_secrets() {
    let (observer, queue) = ConsoleObserver::new();
    let rings = ActivityRings::new(100, 1 << 20, 32);
    for activity in activities() {
        observer.admitted(&record(1), &activity);
        assert_eq!(ActivitySeed::of(&activity).attachments, [1, 0, 0, 0]);
    }
    observer.produced(
        &record(1),
        ProducerFacts {
            source: "https://user:fixture-reply-token@orders.example/a?key=fixture-hmac-secret#x",
            hint: "hmac:ingress",
            ce_type: "order.created",
            outcome: LaunchClass::Accepted,
        },
    );
    observer.refused(
        &record(1),
        "custom",
        None,
        "fixture message text fixture-channel-key",
    );
    drop(observer);
    let mut observations = Vec::new();
    queue
        .drain(|observation| observations.push(observation))
        .await;
    assert_eq!(observations.len(), 4, "{observations:?}");
    let mut dump = String::new();
    for observation in observations {
        dump.push_str(&serde_json::to_string(&observation).unwrap());
        match observation {
            Observation::Routed { record, entry_seed } => rings.admitted(record, *entry_seed),
            Observation::Refused { record, seed } => rings.refused(record, seed),
            Observation::Producer {
                record,
                source_uri,
                hint,
                ce_type,
                outcome,
            } => rings.produced(
                record,
                Seen {
                    source_uri,
                    hint,
                    ce_type,
                    outcome,
                    at: OffsetDateTime::now_utc(),
                },
            ),
            other => panic!("unexpected {other:?}"),
        }
    }
    dump.push_str(
        &serde_json::to_string(&rings.entries(&record(1), &ActivityFilter::default(), usize::MAX))
            .unwrap(),
    );
    dump.push_str(&serde_json::to_string(&rings.producers(&record(1))).unwrap());
    let mut definition: ChannelDefinition = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    definition.secrets.channel_key = Some(Secret::new("fixture-channel-key"));
    definition.secrets.hmac_secrets = vec![("ingress".into(), Secret::new("fixture-hmac-secret"))];
    dump.push_str(
        &serde_json::to_string(&ChannelSummary::of(
            &definition,
            2_048,
            true,
            &std::collections::BTreeSet::new(),
        ))
        .unwrap(),
    );
    assert_eq!(rings.totals().entries, 3);
    for leak in LEAKS {
        assert!(!dump.contains(leak), "{leak} found in {dump}");
    }
}

#[tokio::test]
async fn unauthenticated_custom_requests_never_enter_a_ring() {
    let secret = Secret::new(uuid::Uuid::new_v4().to_string());
    let fixture = custom(&secret).await;
    let memory = Arc::new(ConsoleMemory::new(
        &ConsoleMemoryConfig::default(),
        cmaeg::console::memory::Sources {
            heads: Arc::new(
                cmaeg::records::RecordClient::new("http://127.0.0.1:9", "unused").unwrap(),
            ),
            reader: Arc::new(NoRecords),
        },
        schedule(),
    ));
    let (observer, queue) = ConsoleObserver::new();
    // Never ready: the index never lists the unreachable CMA.
    memory.spawn(queue, Readiness::initialized());
    let observer: Arc<dyn ActivityObserver> = Arc::new(observer);
    let mut state: IngressState = fixture
        .state
        .clone()
        .with_activity_observer(observer.clone());
    state.launcher = Arc::new(ObservedLauncher::new(state.launcher.clone(), observer));
    let event = json!({"specversion":"1.0","id":"evt-1","type":"order.created","source":"https://orders.example/a","data":{"a":1}});
    let forged = Secret::new("not-the-channel-secret");
    for secret in [None, Some(&forged)] {
        let response = send_custom(
            &fixture,
            state.clone(),
            cloud_event(&fixture, secret, &event),
        )
        .await;
        assert_eq!(response.status(), 401);
    }
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(memory.activity_totals().entries, 0);
    assert_eq!(memory.activity_totals().channels, 0);
    let response = send_custom(
        &fixture,
        state,
        cloud_event(&fixture, Some(&secret), &event),
    )
    .await;
    assert_eq!(response.status(), 202);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while memory.activity_totals().entries == 0 {
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(memory.activity_totals().entries, 1);
}
