//! References bound to a non-default consumer, and the sealed lineage of a
//! hand-off chain.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::{
    channel::ReplyChoice,
    ids::{ChannelId, ConsumerId, EventId, MessageRef, RecordId},
    seal::{
        EventRef, EventRefPayload, Lineage, MasterKeyRing, MessageRefPayload, OpenError,
        ReplyAddress, RootId,
    },
};
use serde_json::{json, Value};

#[path = "routing_policy/fixture.rs"]
mod policy;

const NOW: i64 = 1_000;

/// Makes one field of a valid lineage invalid.
type Breakage = fn(&mut Lineage);

fn keys() -> MasterKeyRing {
    MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([6; 32]))).unwrap()
}

fn consumer(id: &str) -> ConsumerId {
    ConsumerId::parse(id).unwrap()
}

fn lineage(hop: u8) -> Lineage {
    Lineage {
        root: RootId::derive(
            &RecordId::new("record-1").unwrap(),
            &EventId::new("event-0").unwrap(),
        )
        .as_str()
        .to_owned(),
        hop,
        path: (1..=hop)
            .map(|index| consumer(&format!("csm_00000000000{index}")))
            .collect(),
        reply: ReplyChoice::Default,
    }
}

fn event() -> EventRefPayload {
    EventRefPayload {
        telegram_admission: None,
        v: 1,
        activity_sha256: Some("b".repeat(64)),
        channel_id: ChannelId::new("cmaeg1.ch.consumer-references").unwrap(),
        record_id: RecordId::new("record-1").unwrap(),
        event_id: EventId::new("event-1").unwrap(),
        partition_key: "order-9".into(),
        conversation_id: format!("conv_{}", "1".repeat(32)).parse().unwrap(),
        expires_at: NOW + 1_800,
        reply: ReplyAddress::None,
        consumer: Some(consumer("csm_00000000000a")),
        lineage: Some(lineage(2)),
    }
}

fn message() -> MessageRefPayload {
    MessageRefPayload {
        discord_message: None,
        native_lark_effect: None,
        whatsapp_reaction: None,
        lark_effect: None,
        telegram_effect: None,
        channel_id: ChannelId::new("cmaeg1.ch.consumer-references").unwrap(),
        event_id: EventId::new("event-1").unwrap(),
        platform_message_id: "42".into(),
        route_id: "route-1".into(),
        interaction_record_id: None,
        relay_message: None,
        consumer: Some(consumer("csm_00000000000a")),
    }
}

#[test]
fn consumer_bound_event_refs_round_trip_their_consumer_and_lineage() {
    let payload = event();
    payload.validate().unwrap();
    let wire = serde_json::to_value(&payload).unwrap();
    assert_eq!(wire["consumer"], "csm_00000000000a");
    assert_eq!(wire["lineage"]["hop"], 2);
    assert_eq!(
        wire["lineage"]["path"],
        json!(["csm_000000000001", "csm_000000000002"])
    );
    assert_eq!(wire["lineage"]["reply"], "default");
    let reference = EventRef::seal(&payload, &keys()).unwrap();
    assert_eq!(reference.open(&keys(), NOW).unwrap().payload, payload);
}

#[test]
fn a_default_publish_policy_has_a_representable_sealed_lineage() {
    let mut definition = policy::routed();
    definition["routing"]["default_consumer"]["publish"] =
        json!({"enabled": true, "targets": [policy::REVIEWER]});
    let definition = policy::definition(definition);
    definition.validate().unwrap();
    let publisher = definition.consumer(None).unwrap();
    assert!(publisher.publish.enabled);
    let mut payload = event();
    payload.consumer = Some(publisher.publish.targets[0].clone());
    let mut chain = lineage(1);
    chain.path = vec![publisher.id.clone()];
    payload.lineage = Some(chain);
    payload.validate().unwrap();
    let reference = EventRef::seal(&payload, &keys()).unwrap();
    assert_eq!(reference.open(&keys(), NOW).unwrap().payload, payload);
}

#[test]
fn default_is_a_distinct_publisher_at_any_bounded_hop() {
    for hop in 1..=4 {
        for position in 0..usize::from(hop) {
            let mut payload = event();
            let mut chain = lineage(hop);
            chain.path[position] = ConsumerId::default_consumer();
            payload.lineage = Some(chain);
            let reference = EventRef::seal(&payload, &keys()).unwrap();
            assert_eq!(reference.open(&keys(), NOW).unwrap().payload, payload);
        }
    }
}

#[test]
fn a_default_publishers_reference_still_requires_an_authentic_unexpired_seal() {
    let mut payload = event();
    let mut chain = lineage(1);
    chain.path = vec![ConsumerId::default_consumer()];
    payload.lineage = Some(chain);
    let reference = EventRef::seal(&payload, &keys()).unwrap();
    assert_eq!(
        reference
            .open(&keys(), payload.expires_at - 1)
            .unwrap()
            .payload,
        payload
    );
    assert!(matches!(
        reference.open(&keys(), payload.expires_at),
        Err(OpenError::Expired)
    ));

    // A forged, otherwise valid default-origin chain cannot widen the recipient,
    // channel, record or expiry without the gateway's sealing key.
    let attacker = MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([7; 32]))).unwrap();
    payload.consumer = None;
    payload.channel_id = ChannelId::new("cmaeg1.ch.another-owner").unwrap();
    payload.record_id = RecordId::new("another-owner-record").unwrap();
    payload.expires_at += 3_600;
    payload.validate().unwrap();
    let forged = EventRef::seal(&payload, &attacker).unwrap();
    assert!(matches!(
        forged.open(&keys(), NOW),
        Err(OpenError::Tampered)
    ));
}

#[test]
fn absent_consumer_fields_are_omitted_from_references() {
    let mut payload = event();
    payload.consumer = None;
    payload.lineage = None;
    let wire = serde_json::to_value(&payload).unwrap();
    assert!(wire.get("consumer").is_none() && wire.get("lineage").is_none());
    let legacy = EventRef::seal(&payload, &keys()).unwrap();
    assert_eq!(legacy.open(&keys(), NOW).unwrap().payload, payload);
    let mut reference = message();
    reference.consumer = None;
    assert!(serde_json::to_value(&reference)
        .unwrap()
        .get("consumer")
        .is_none());
}

#[test]
fn consumer_bound_message_refs_round_trip_their_consumer() {
    let payload = message();
    payload.validate().unwrap();
    let reference = MessageRef::seal(&payload, &keys()).unwrap();
    assert_eq!(reference.open(&keys()).unwrap().payload, payload);
}

#[test]
fn references_never_name_the_default_consumer() {
    let mut payload = event();
    payload.consumer = Some(ConsumerId::default_consumer());
    assert_eq!(payload.validate().unwrap_err().pointer, "/consumer");
    let reference = EventRef::seal(&payload, &keys()).unwrap();
    assert!(matches!(
        reference.open(&keys(), NOW),
        Err(OpenError::Format)
    ));

    let mut payload = message();
    payload.consumer = Some(ConsumerId::default_consumer());
    assert_eq!(payload.validate().unwrap_err().pointer, "/consumer");
    let reference = MessageRef::seal(&payload, &keys()).unwrap();
    assert!(matches!(reference.open(&keys()), Err(OpenError::Format)));
}

#[test]
fn a_hand_off_lineage_is_bounded_and_names_distinct_publishers() {
    for hop in 1..=4 {
        lineage(hop).validate().unwrap();
    }
    let cases: [(&str, Breakage); 7] = [
        ("/hop", |lineage| lineage.hop = 0),
        ("/hop", |lineage| {
            lineage.hop = 5;
            lineage.path.push(consumer("csm_000000000005"));
        }),
        ("/path", |lineage| lineage.hop = 1),
        ("/path", |lineage| lineage.path[1] = lineage.path[0].clone()),
        ("/path", |lineage| {
            lineage.path = vec![ConsumerId::default_consumer(); 2]
        }),
        ("/root", |lineage| {
            lineage.root = lineage.root.to_uppercase()
        }),
        ("/root", |lineage| lineage.root.truncate(31)),
    ];
    for (pointer, break_it) in cases {
        let mut broken = lineage(2);
        break_it(&mut broken);
        assert_eq!(broken.validate().unwrap_err().pointer, pointer);
        let mut payload = event();
        payload.lineage = Some(broken);
        assert_eq!(
            payload.validate().unwrap_err().pointer,
            format!("/lineage{pointer}")
        );
        let reference = EventRef::seal(&payload, &keys()).unwrap();
        assert!(matches!(
            reference.open(&keys(), NOW),
            Err(OpenError::Format)
        ));
    }
}

#[test]
fn a_lineage_refuses_unknown_fields() {
    let mut wire = serde_json::to_value(lineage(1)).unwrap();
    wire["publisher"] = json!("csm_00000000000a");
    assert!(serde_json::from_value::<Lineage>(wire).is_err());
    let mut wire: Value = serde_json::to_value(event()).unwrap();
    wire["lineage"]["path"] = json!(["reviewer"]);
    assert!(serde_json::from_value::<EventRefPayload>(wire).is_err());
}

#[test]
fn root_ids_are_scoped_to_the_record() {
    let event = EventId::new("order-created-1").unwrap();
    let first = RootId::derive(&RecordId::new("record-1").unwrap(), &event);
    let second = RootId::derive(&RecordId::new("record-2").unwrap(), &event);
    assert_ne!(first, second);
    assert_eq!(
        first,
        RootId::derive(&RecordId::new("record-1").unwrap(), &event)
    );
    for root in [&first, &second] {
        assert_eq!(root.as_str().len(), 32);
        assert!(root
            .as_str()
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    }
    assert_ne!(
        first,
        RootId::derive(
            &RecordId::new("record-1").unwrap(),
            &EventId::new("order-created-2").unwrap()
        )
    );
}

#[test]
fn root_ids_hash_the_documented_domain_and_separators() {
    // SHA-256("cmaeg.root.v1\x1frecord-1\x1fevent-0"), first 16 bytes,
    // computed independently of this crate.
    let root = RootId::derive(
        &RecordId::new("record-1").unwrap(),
        &EventId::new("event-0").unwrap(),
    );
    assert_eq!(root.as_str(), ROOT_VECTOR);
}

const ROOT_VECTOR: &str = "abd900dfc851fc9138d35d28db77d523";
