//! Hand-off identities and sealed context, independent of provider execution.
use super::routing_fixture;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg::tools::{PreparedPublish, PublishEventRequest, PublishOrigin};
use cmaeg_core::{
    activity::{ActivityKind, ActorKind, ConsumerTarget, SourceKind},
    channel::{ChannelDefinition, Platform, ReplyChoice, Secret},
    ids::{
        compact_partition_key, ChannelId, ConsumerId, ConsumerName, ConversationId, EventId,
        IdempotencyKey, RecordId,
    },
    seal::{EventRef, EventRefPayload, MasterKeyRing, ReplyAddress, RootId},
};
use serde_json::{json, Value};
use time::OffsetDateTime;

fn keys() -> MasterKeyRing {
    MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([7; 32]))).unwrap()
}
fn definition() -> ChannelDefinition {
    routing_fixture::definition(routing_fixture::routed())
}
fn request() -> PublishEventRequest {
    serde_json::from_value(json!({"targets":["reviewer"],"text":"private-handoff-text","data":{"z":3,"a":"private-handoff-data"},"idempotency_key":"private-handoff-key"})).unwrap()
}
fn parent(reply: ReplyAddress) -> EventRefPayload {
    EventRefPayload {
        v: 1,
        activity_sha256: None,
        channel_id: ChannelId::new("cmaeg1.ch.fixture").unwrap(),
        record_id: RecordId::new("record-1").unwrap(),
        event_id: EventId::new("parent-event").unwrap(),
        partition_key: "fixed".into(),
        conversation_id: ConversationId::new("parent-conversation").unwrap(),
        expires_at: 3000,
        reply,
        consumer: None,
        lineage: None,
    }
}
fn target(id: &str, name: &str) -> ConsumerTarget {
    ConsumerTarget {
        id: ConsumerId::parse(id).unwrap(),
        name: ConsumerName::parse(name).unwrap(),
    }
}
fn build(
    definition: &ChannelDefinition,
    parent: &EventRefPayload,
    publisher: &ConsumerId,
    request: &PublishEventRequest,
    targets: Vec<ConsumerTarget>,
) -> PreparedPublish {
    PreparedPublish::build(
        PublishOrigin {
            definition,
            parent,
            publisher: definition.consumer(Some(publisher)).unwrap(),
            keys: &keys(),
        },
        request,
        targets,
        request.event_id(parent, publisher),
        OffsetDateTime::from_unix_timestamp(1000).unwrap(),
    )
    .unwrap()
}
fn initial(reply: ReplyAddress) -> PreparedPublish {
    build(
        &definition(),
        &parent(reply),
        &ConsumerId::default_consumer(),
        &request(),
        vec![target(routing_fixture::REVIEWER, "reviewer")],
    )
}

#[test]
fn publish_request_bounds_are_strict_without_echoing_private_input() {
    request().validate(8).unwrap();
    let mut value = json!({"targets":["reviewer"],"text":"x"});
    for field in ["text", "targets"] {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<PublishEventRequest>(missing).is_err());
    }
    value["unexpected"] = true.into();
    assert!(serde_json::from_value::<PublishEventRequest>(value).is_err());
    for bad in [
        Value::Array(vec![]),
        json!(3),
        json!("private-handoff-data"),
    ] {
        assert!(serde_json::from_value::<PublishEventRequest>(
            json!({"targets":["reviewer"],"text":"x","data":bad})
        )
        .is_err());
    }
    for size in [0, 9] {
        let mut request = request();
        request.targets = vec!["reviewer".into(); size];
        assert_eq!(request.validate(8).unwrap_err().status, 400);
    }
    let mut request = request();
    request.text = "é".repeat(8192);
    request.validate(8).unwrap();
    request.text.push('x');
    assert_eq!(request.validate(8).unwrap_err().status, 413);
    request.text.clear();
    assert_eq!(request.validate(8).unwrap_err().status, 400);
    request.text = "x".into();
    request.data = Some(serde_json::from_value(json!({"a":"x".repeat(8184)})).unwrap());
    request.validate(8).unwrap();
    request.data = Some(serde_json::from_value(json!({"a":"x".repeat(8185)})).unwrap());
    assert_eq!(request.validate(8).unwrap_err().status, 413);
    request.data = None;
    for kind in ["A.z_9-", &"x".repeat(128)] {
        request.event_type = Some(kind.into());
        request.validate(8).unwrap();
    }
    for kind in ["", "é", "x y", &"x".repeat(129)] {
        request.event_type = Some(kind.into());
        assert_eq!(request.validate(8).unwrap_err().status, 400);
    }
    request.event_type = None;
    for key in [" ", &"x".repeat(128)] {
        request.idempotency_key = Some(key.into());
        request.validate(8).unwrap();
    }
    for key in ["", "\n", "é", &"x".repeat(129)] {
        request.idempotency_key = Some(key.into());
        assert_eq!(request.validate(8).unwrap_err().status, 400);
    }
}

#[test]
fn stable_ids_include_the_record_parent_publisher_and_key() {
    let request = request();
    let mut parent = parent(ReplyAddress::None);
    let publisher = ConsumerId::default_consumer();
    let original = request.event_id(&parent, &publisher);
    assert_eq!(original.as_str(), "hof_1c3fbb75e4216b1d0781ebc70400d02f");
    assert_eq!(original, request.event_id(&parent, &publisher));
    assert!(original.as_str().starts_with("hof_"));
    assert_eq!(original.as_str().len(), 36);
    assert_eq!(
        IdempotencyKey::for_consumer(&original, &publisher),
        IdempotencyKey::for_consumer(&request.event_id(&parent, &publisher), &publisher)
    );
    parent.record_id = RecordId::new("other-record").unwrap();
    assert_ne!(original, request.event_id(&parent, &publisher));
    parent.record_id = RecordId::new("record-1").unwrap();
    parent.event_id = EventId::new("other-parent").unwrap();
    assert_ne!(original, request.event_id(&parent, &publisher));
    parent.event_id = EventId::new("parent-event").unwrap();
    assert_ne!(
        original,
        request.event_id(
            &parent,
            &ConsumerId::parse(routing_fixture::REVIEWER).unwrap()
        )
    );
    let mut other = request.clone();
    other.idempotency_key = Some("other-key".into());
    assert_ne!(original, other.event_id(&parent, &publisher));
    other.idempotency_key = None;
    assert_ne!(
        other.event_id(&parent, &publisher),
        other.event_id(&parent, &publisher)
    );
}

#[test]
fn a_two_hop_chain_keeps_its_root_and_seals_each_publishers_path() {
    let first = initial(ReplyAddress::None);
    let second = build(
        &definition(),
        &first.payload,
        &ConsumerId::parse(routing_fixture::REVIEWER).unwrap(),
        &request(),
        vec![target(routing_fixture::NYX, "nyx")],
    );
    let first_lineage = first.payload.lineage.as_ref().unwrap();
    let second_lineage = second.payload.lineage.as_ref().unwrap();
    assert_eq!(first_lineage.hop, 1);
    assert_eq!(first_lineage.path, [ConsumerId::default_consumer()]);
    assert_eq!(second_lineage.hop, 2);
    assert_eq!(second_lineage.root, first_lineage.root);
    assert_eq!(
        second_lineage.path,
        [
            ConsumerId::default_consumer(),
            ConsumerId::parse(routing_fixture::REVIEWER).unwrap()
        ]
    );
    assert_eq!(
        PreparedPublish::root(&first.payload).unwrap().as_str(),
        first_lineage.root
    );
    assert_eq!(
        RootId::derive(
            &first.payload.record_id,
            &EventId::new("parent-event").unwrap()
        )
        .as_str(),
        first_lineage.root
    );
    assert!(second.payload.verifies_activity(&second.activity));
    let reference = EventRef::seal(&second.payload, &keys()).unwrap();
    assert_eq!(
        reference.open(&keys(), 1000).unwrap().payload,
        second.payload
    );
    let cause = second
        .activity
        .routing
        .as_ref()
        .unwrap()
        .causation
        .as_ref()
        .unwrap();
    assert_eq!(cause.parent_event_id, first.activity.event_id);
    assert_eq!(cause.hop, 2);
    assert_eq!(cause.origin_source_id.as_str(), "custom");
    assert_eq!(second.payload.expires_at, 2800);
    assert!(second.payload.consumer.is_none());
}

#[test]
fn activities_keep_origin_source_kind_and_safe_reply_addresses() {
    let native = initial(ReplyAddress::Telegram {
        bot_id: "77".into(),
        chat_id: "-42".into(),
        message_id: "8".into(),
        thread_id: Some("9".into()),
    });
    assert!(
        matches!(native.activity.source,SourceKind::Telegram{platform:Platform::Telegram,ref bot_id} if bot_id=="77")
    );
    assert_eq!(native.activity.conversation.id, "-42");
    assert_eq!(native.activity.conversation.thread_id.as_deref(), Some("9"));
    assert_eq!(native.activity.actor.kind, ActorKind::Bot);
    assert_eq!(native.activity.actor.id, "consumer:default");
    assert_eq!(native.activity.actor.display_name.as_deref(), Some("lead"));
    let relay = initial(ReplyAddress::NyxidRelay {
        message_id: "msg".into(),
        route_id: routing_fixture::ROUTE.into(),
        platform: Platform::Lark,
        reply_token: Secret::new("private-relay-token"),
        reply_token_expires_at: 9000,
    });
    assert!(matches!(
        relay.activity.source,
        SourceKind::NyxidRelay {
            platform: Platform::Lark,
            ..
        }
    ));
    assert!(relay.activity.conversation.id.starts_with("handoff_"));
    assert!(
        matches!(&relay.payload.reply,ReplyAddress::NyxidRelay{reply_token,reply_token_expires_at:1,..} if reply_token.as_str().is_empty())
    );
    let sync = initial(ReplyAddress::Sync {
        waiter: "original-http-waiter".into(),
    });
    assert_eq!(sync.payload.reply, ReplyAddress::None);
    assert_eq!(sync.activity.source, SourceKind::Custom);
    let data = match &native.activity.kind {
        ActivityKind::Event { ce, attachments } => {
            assert!(attachments.is_empty());
            assert_eq!(ce.specversion, "1.0");
            assert_eq!(ce.r#type, "cmaeg.handoff");
            assert_eq!(ce.source, "urn:cmaeg:consumer:default");
            assert_eq!(ce.datacontenttype.as_deref(), Some("application/json"));
            ce.data.as_ref().unwrap()
        }
        _ => panic!("expected event"),
    };
    assert_eq!(data["text"], "private-handoff-text");
    assert_eq!(data["data"]["a"], "private-handoff-data");
}

#[test]
fn compact_partition_is_verbatim_and_documents_the_default_limitation() {
    let definition = definition();
    let publisher = ConsumerId::parse(routing_fixture::REVIEWER).unwrap();
    for raw in ["x".repeat(512), "x".repeat(513)] {
        let mut parent = parent(ReplyAddress::None);
        parent.partition_key = compact_partition_key(&raw);
        let built = build(
            &definition,
            &parent,
            &publisher,
            &request(),
            vec![target("default", "lead")],
        );
        assert_eq!(built.payload.partition_key, parent.partition_key);
        let direct = ConversationId::derive(
            &keys(),
            &parent.channel_id,
            definition.partition.version,
            &raw,
        );
        assert_eq!(built.payload.conversation_id == direct, raw.len() <= 512);
        let reviewer = ConsumerId::parse(routing_fixture::REVIEWER).unwrap();
        assert_eq!(
            ConversationId::derive_for_consumer(
                &keys(),
                &parent.channel_id,
                1,
                &reviewer,
                &compact_partition_key(&raw)
            ),
            ConversationId::derive_for_consumer(
                &keys(),
                &parent.channel_id,
                1,
                &reviewer,
                &built.payload.partition_key
            )
        );
    }
}

#[test]
fn reply_defaults_follow_the_channels_fallback_and_explicit_choice_wins() {
    let mut value = routing_fixture::routed();
    value["routing"]["default_reply"] =
        json!({"type":"telegram_chat","source_id":"telegram","chat_id":"-42"});
    let definition = routing_fixture::definition(value);
    let parent = parent(ReplyAddress::None);
    let publisher = ConsumerId::default_consumer();
    let mut request = request();
    let built = build(
        &definition,
        &parent,
        &publisher,
        &request,
        vec![target(routing_fixture::REVIEWER, "reviewer")],
    );
    assert_eq!(built.payload.lineage.unwrap().reply, ReplyChoice::Default);
    request.reply = Some(ReplyChoice::Origin);
    let built = build(
        &definition,
        &parent,
        &publisher,
        &request,
        vec![target(routing_fixture::REVIEWER, "reviewer")],
    );
    assert_eq!(built.payload.lineage.unwrap().reply, ReplyChoice::Origin);
    assert_eq!(
        initial(ReplyAddress::None).payload.lineage.unwrap().reply,
        ReplyChoice::Origin
    );
}

#[test]
fn builder_refuses_unrouted_or_malformed_sealed_inputs() {
    let mut definition = definition();
    let mut parent = parent(ReplyAddress::None);
    let request = request();
    let invoke = |definition: &ChannelDefinition, parent: &EventRefPayload, targets| {
        PreparedPublish::build(
            PublishOrigin {
                definition,
                parent,
                publisher: definition.consumer(None).unwrap(),
                keys: &keys(),
            },
            &request,
            targets,
            request.event_id(parent, &ConsumerId::default_consumer()),
            OffsetDateTime::from_unix_timestamp(1000).unwrap(),
        )
    };
    let targets = || vec![target(routing_fixture::REVIEWER, "reviewer")];
    let routing = definition.routing.take();
    assert_eq!(
        invoke(&definition, &parent, targets())
            .err()
            .unwrap()
            .detail
            .as_deref(),
        Some("routing_required")
    );
    definition.routing = routing;
    assert_eq!(
        invoke(&definition, &parent, vec![])
            .err()
            .unwrap()
            .detail
            .as_deref(),
        Some("invalid_published_activity")
    );
    parent.partition_key.clear();
    assert_eq!(
        invoke(&definition, &parent, targets())
            .err()
            .unwrap()
            .detail
            .as_deref(),
        Some("invalid_published_reference")
    );
    parent.partition_key = "fixed".into();
    parent.lineage = initial(ReplyAddress::None).payload.lineage;
    // The publisher cannot appear twice, even in a hand-built internal input.
    assert_eq!(
        invoke(&definition, &parent, targets())
            .err()
            .unwrap()
            .detail
            .as_deref(),
        Some("invalid_lineage")
    );
    parent.lineage.as_mut().unwrap().root = "malformed".into();
    assert_eq!(
        PreparedPublish::root(&parent)
            .unwrap_err()
            .detail
            .as_deref(),
        Some("event_ref_invalid")
    );
}
