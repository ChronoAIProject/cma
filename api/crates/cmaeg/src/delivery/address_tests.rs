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

#[test]
fn chat_addresses_class_as_chat_and_sinks_as_custom() {
    let relay = ReplyAddress::NyxidRelay {
        message_id: "1".into(),
        route_id: "route-1".into(),
        platform: cmaeg_core::channel::Platform::Lark,
        reply_token: cmaeg_core::channel::Secret::new("token"),
        reply_token_expires_at: 1,
    };
    let telegram = ReplyAddress::Telegram {
        bot_id: "77".into(),
        chat_id: "42".into(),
        message_id: "5".into(),
        thread_id: None,
    };
    for address in [relay, telegram] {
        assert_eq!(
            EffectiveAddress::Origin(address).class(),
            Some(AddressClass::Chat)
        );
    }
    let sinks = [
        ReplyAddress::Sync {
            waiter: "waiter".into(),
        },
        ReplyAddress::ResultWebhook {
            url: "https://receiver.example/events".into(),
            hmac_key_id: "key-1".into(),
        },
        ReplyAddress::None,
    ];
    for address in sinks {
        assert_eq!(
            EffectiveAddress::Origin(address).class(),
            Some(AddressClass::Custom)
        );
    }
    let rejected = EffectiveAddress::Rejected {
        reason: DEFAULT_REPLY_UNAVAILABLE,
    };
    assert_eq!(rejected.class(), None);
    // A default chat is a chat whatever the origin; default sinks are custom.
    let default_chat = EffectiveAddress::DefaultTelegramChat {
        bot_id: "77".into(),
        chat_id: "42".into(),
        thread_id: None,
    };
    let default_relay = EffectiveAddress::DefaultRelayConversation {
        platform: cmaeg_core::channel::Platform::Lark,
        route_id: "route-1".into(),
    };
    for address in [default_chat, default_relay] {
        assert_eq!(address.class(), Some(AddressClass::Chat));
    }
    assert_eq!(
        EffectiveAddress::DefaultSinks.class(),
        Some(AddressClass::Custom)
    );
}

#[test]
fn only_telegram_cards_on_a_handoff_are_detected() {
    let buttons: OutboundMessage = serde_json::from_value(json!({"text":"Choose","blocks":[
        {"type":"buttons","id":"choice","options":[{"id":"a","label":"A"},{"id":"b","label":"B"}]}]}))
    .unwrap();
    let text: OutboundMessage = serde_json::from_value(json!({"text":"Plain"})).unwrap();
    let telegram = |lineage: bool, platform| {
        let mut event = payload(true, lineage);
        event.reply = ReplyAddress::NyxidRelay {
            message_id: "1".into(),
            route_id: "route-1".into(),
            platform,
            reply_token: cmaeg_core::channel::Secret::new("token"),
            reply_token_expires_at: 1,
        };
        event
    };
    use cmaeg_core::channel::Platform;
    assert!(handoff_controls(
        &telegram(true, Platform::Telegram),
        &buttons
    ));
    assert!(!handoff_controls(
        &telegram(true, Platform::Telegram),
        &text
    ));
    assert!(!handoff_controls(
        &telegram(false, Platform::Telegram),
        &buttons
    ));
    assert!(!handoff_controls(&telegram(true, Platform::Lark), &buttons));
    let mut native = payload(true, true);
    native.reply = ReplyAddress::Telegram {
        bot_id: "77".into(),
        chat_id: "42".into(),
        message_id: "5".into(),
        thread_id: None,
    };
    assert!(handoff_controls(&native, &buttons));
}

#[test]
fn an_unavailable_default_reply_is_counted_by_its_configured_transport() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let metrics = recorder.handle();
    let mut telegram = definition(true);
    telegram.routing.as_mut().unwrap().default_reply = Some(
        serde_json::from_value(
            json!({"type":"telegram_chat","source_id":"telegram","chat_id":"42"}),
        )
        .unwrap(),
    );
    metrics::with_local_recorder(&recorder, || {
        count_unavailable(&definition(true));
        count_unavailable(&telegram);
        count_unavailable(&telegram);
    });
    let rendered = metrics.render();
    for line in [
        "cmaeg_default_reply_total{transport=\"none\",outcome=\"unavailable\"} 1",
        "cmaeg_default_reply_total{transport=\"telegram\",outcome=\"unavailable\"} 2",
    ] {
        assert!(rendered.contains(line), "{rendered}");
    }
}
