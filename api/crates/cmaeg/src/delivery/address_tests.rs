use super::*;
use cmaeg_core::{
    channel::ReplyChoice,
    ids::{ChannelId, ConsumerId},
    seal::{Lineage, ReplyAddress},
};
use serde_json::{json, Value};

const REVIEWER: &str = "csm_00000000000a";

fn definition(routing: bool) -> ChannelDefinition {
    let mut value: Value = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    if routing {
        value["routing"] = json!({"version": 1, "default_consumer": {"name": "lead"},
            "default_targets": ["default"]});
    }
    serde_json::from_value(value).unwrap()
}

fn payload(consumer: bool, lineage: bool) -> EventRefPayload {
    EventRefPayload {
        v: 1,
        activity_sha256: None,
        channel_id: ChannelId::new("cmaeg1.ch.address").unwrap(),
        record_id: "record-1".parse().unwrap(),
        event_id: "msg-1".parse().unwrap(),
        partition_key: "msg-1".into(),
        conversation_id: "conversation-1".parse().unwrap(),
        expires_at: i64::MAX,
        reply: ReplyAddress::None,
        consumer: consumer.then(|| ConsumerId::parse(REVIEWER).unwrap()),
        lineage: lineage.then(|| Lineage {
            root: "0123456789abcdef0123456789abcdef".into(),
            hop: 1,
            path: vec![ConsumerId::parse(REVIEWER).unwrap()],
            reply: ReplyChoice::Origin,
        }),
    }
}

/// (routing, flag, consumer, lineage) → whether a routed turn's output or a
/// tool command is routed.
const ROUTED: [(bool, bool, bool, bool, bool); 16] = [
    (false, false, false, false, false),
    (false, false, false, true, false),
    (false, false, true, false, false),
    (false, false, true, true, false),
    (false, true, false, false, false),
    (false, true, false, true, false),
    (false, true, true, false, false),
    (false, true, true, true, false),
    (true, false, false, false, false),
    (true, false, false, true, true),
    (true, false, true, false, true),
    (true, false, true, true, true),
    (true, true, false, false, true),
    (true, true, false, true, true),
    (true, true, true, false, true),
    (true, true, true, true, true),
];

#[test]
fn routing_send_truth_table() {
    for (routing, flag, consumer, lineage, routed) in ROUTED {
        let def = definition(routing);
        let payload = payload(consumer, lineage);
        let row = format!("routing={routing} flag={flag} consumer={consumer} lineage={lineage}");
        assert!(
            !routing_send(
                &def,
                &payload,
                flag,
                SendKind::Output { routed_turn: false }
            ),
            "an unrouted turn's output is never routed: {row}"
        );
        assert_eq!(
            routing_send(&def, &payload, flag, SendKind::Output { routed_turn: true }),
            routed,
            "routed output: {row}"
        );
        assert_eq!(
            routing_send(&def, &payload, flag, SendKind::Tool),
            routed,
            "tool: {row}"
        );
    }
}

#[test]
fn routing_send_for_message_truth_table() {
    // (routing, flag, consumer) → routed.
    let table = [
        (false, false, false, false),
        (false, false, true, false),
        (false, true, false, false),
        (false, true, true, false),
        (true, false, false, false),
        (true, false, true, true),
        (true, true, false, true),
        (true, true, true, true),
    ];
    for (routing, flag, consumer, routed) in table {
        let message = MessageRefPayload {
            channel_id: ChannelId::new("cmaeg1.ch.address").unwrap(),
            event_id: "msg-1".parse().unwrap(),
            platform_message_id: "platform-1".into(),
            route_id: "route-1".into(),
            interaction_record_id: None,
            consumer: consumer.then(|| ConsumerId::parse(REVIEWER).unwrap()),
        };
        assert_eq!(
            routing_send_for_message(&definition(routing), &message, flag),
            routed,
            "routing={routing} flag={flag} consumer={consumer}"
        );
    }
}
