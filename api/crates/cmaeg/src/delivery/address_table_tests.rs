//! The destination table: every requested destination against an origin's
//! reply capability, the default destination's availability, the flag and
//! the send kind, and revocation through later snapshots.
use super::*;
use cmaeg_core::{
    channel::ReplyChoice,
    ids::{ChannelId, ConsumerId},
    seal::{Lineage, ReplyAddress},
};
use serde_json::{json, Value};

const REVIEWER: &str = "csm_00000000000a";
const ROUTE: &str = "6f1c9a3e-2b4d-4c5e-8f70-112233445566";
const TOOL: SendKind = SendKind::Tool;
const ROUTED_OUTPUT: SendKind = SendKind::Output { routed_turn: true };

#[test]
fn unmarked_default_consumers_never_redirect_a_default_request() {
    let mut payload = event(chat_origin());
    payload.consumer = None;
    payload.lineage = None;
    for routing in [false, true] {
        let def = channel(Some(telegram_default()), |value| {
            if !routing {
                value.as_object_mut().unwrap().remove("routing");
            }
        });
        for flag in [false, true] {
            for kind in [TOOL, ROUTED_OUTPUT, SendKind::Output { routed_turn: false }] {
                for choice in [ReplyChoice::Origin, ReplyChoice::Default] {
                    let actual = effective(&def, &payload, choice, false, flag, kind);
                    let expected = if choice == ReplyChoice::Origin
                        || matches!(kind, SendKind::Output { routed_turn: false })
                    {
                        EffectiveAddress::Origin(payload.reply.clone())
                    } else if routing && flag {
                        default_chat()
                    } else {
                        EffectiveAddress::Rejected {
                            reason: DEFAULT_REPLY_UNAVAILABLE,
                        }
                    };
                    assert_eq!(
                        actual, expected,
                        "routing={routing} flag={flag} kind={kind:?} choice={choice:?}"
                    );
                }
            }
        }
    }
}

/// A routed channel with a native bot, a Lark relay and a custom source,
/// whose `routing.default_reply` is `default_reply`, after `edit`.
fn channel(default_reply: Option<Value>, edit: impl FnOnce(&mut Value)) -> ChannelDefinition {
    let mut value: Value = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    value["sources"] = json!([
        {"type":"custom","auth":{"type":"hmac","key_id":"key-1"}},
        {"type":"telegram","bot_id":"77","admission":{"type":"all"}},
        {"type":"nyxid_relay","issuer":"https://nyx.example","jwks_url":"https://nyx.example/jwks",
            "key_id":"key-1","route_ids":[ROUTE],"platform":"lark","admission":{"type":"all"}}
    ]);
    value["secrets"]["telegram_bot_token"] = "77:token".into();
    value["secrets"]["channel_key"] = "channel-key".into();
    value["routing"] = json!({"version": 1, "default_consumer": {"name": "lead"},
        "default_targets": ["default"]});
    if let Some(reply) = default_reply {
        value["routing"]["default_reply"] = reply;
    }
    edit(&mut value);
    serde_json::from_value(value).unwrap()
}

fn telegram_default() -> Value {
    json!({"type":"telegram_chat","source_id":"telegram","chat_id":"-100","thread_id":"7"})
}

fn relay_default() -> Value {
    json!({"type":"relay_conversation","source_id":"relay_lark","route_id":ROUTE})
}

fn with_sinks(value: &mut Value) {
    value["reply"]["sinks"] = json!([{"type":"result_webhook","url":"https://receiver.example/events","hmac_key_id":"result"}]);
}

/// The default chat `telegram_default` names.
fn default_chat() -> EffectiveAddress {
    EffectiveAddress::DefaultTelegramChat {
        bot_id: "77".into(),
        chat_id: "-100".into(),
        thread_id: Some("7".into()),
    }
}

/// An event bound to the reviewer, answered at `reply`.
fn event(reply: ReplyAddress) -> EventRefPayload {
    EventRefPayload {
        v: 1,
        activity_sha256: None,
        channel_id: ChannelId::new("cmaeg1.ch.address").unwrap(),
        record_id: "record-1".parse().unwrap(),
        event_id: "msg-1".parse().unwrap(),
        partition_key: "msg-1".into(),
        conversation_id: "conversation-1".parse().unwrap(),
        expires_at: i64::MAX,
        reply,
        consumer: Some(ConsumerId::parse(REVIEWER).unwrap()),
        lineage: None,
    }
}

fn chat_origin() -> ReplyAddress {
    ReplyAddress::Telegram {
        bot_id: "77".into(),
        chat_id: "42".into(),
        message_id: "5".into(),
        thread_id: None,
    }
}

fn sync_origin() -> ReplyAddress {
    ReplyAddress::Sync {
        waiter: "waiter".into(),
    }
}

/// `requested` on `def` for `payload`, flag on, Sync waiters registered as
/// `sync_registered`.
fn resolve(
    def: &ChannelDefinition,
    payload: &EventRefPayload,
    requested: ReplyChoice,
    sync_registered: bool,
    kind: SendKind,
) -> EffectiveAddress {
    effective(def, payload, requested, sync_registered, true, kind)
}

#[test]
fn a_reply_capable_origin_takes_origin_requests_whatever_the_default() {
    let capable = [
        (event(chat_origin()), false),
        (event(sync_origin()), true),
        (
            event(ReplyAddress::ResultWebhook {
                url: "https://receiver.example/events".into(),
                hmac_key_id: "result".into(),
            }),
            false,
        ),
    ];
    for default in [None, Some(telegram_default())] {
        let def = channel(default.clone(), |_| {});
        for (payload, registered) in &capable {
            for kind in [TOOL, ROUTED_OUTPUT] {
                assert_eq!(
                    resolve(&def, payload, ReplyChoice::Origin, *registered, kind),
                    EffectiveAddress::Origin(payload.reply.clone()),
                    "default={default:?} origin={:?}",
                    payload.reply
                );
            }
        }
    }
    // No origin is reply-capable on a channel with a result webhook.
    let def = channel(Some(telegram_default()), with_sinks);
    let none = event(ReplyAddress::None);
    assert_eq!(
        resolve(&def, &none, ReplyChoice::Origin, false, TOOL),
        EffectiveAddress::Origin(ReplyAddress::None)
    );
}

#[test]
fn an_origin_that_cannot_take_replies_falls_back_to_the_default() {
    // A Sync origin whose waiter is gone, and no origin without sinks.
    let incapable = [event(sync_origin()), event(ReplyAddress::None)];
    let def = channel(Some(telegram_default()), |_| {});
    for payload in &incapable {
        for kind in [TOOL, ROUTED_OUTPUT] {
            assert_eq!(
                resolve(&def, payload, ReplyChoice::Origin, false, kind),
                default_chat()
            );
        }
    }
    let relay = channel(Some(relay_default()), |_| {});
    assert_eq!(
        resolve(&relay, &incapable[1], ReplyChoice::Origin, false, TOOL),
        EffectiveAddress::DefaultRelayConversation {
            platform: Platform::Lark,
            route_id: ROUTE.into(),
        }
    );
    let sinks = channel(Some(json!({"type":"sinks"})), with_sinks);
    assert_eq!(
        resolve(&sinks, &incapable[0], ReplyChoice::Origin, false, TOOL),
        EffectiveAddress::DefaultSinks
    );
}

#[test]
fn without_a_default_an_incapable_origin_keeps_todays_address() {
    let def = channel(None, |_| {});
    for payload in [event(sync_origin()), event(ReplyAddress::None)] {
        assert_eq!(
            resolve(&def, &payload, ReplyChoice::Origin, false, TOOL),
            EffectiveAddress::Origin(payload.reply.clone())
        );
    }
}

#[test]
fn a_default_request_resolves_the_default_or_is_rejected() {
    let unavailable = EffectiveAddress::Rejected {
        reason: DEFAULT_REPLY_UNAVAILABLE,
    };
    let with_default = channel(Some(telegram_default()), |_| {});
    let without = channel(None, |_| {});
    for (payload, registered) in [
        (event(chat_origin()), false),
        (event(sync_origin()), true),
        (event(sync_origin()), false),
        (event(ReplyAddress::None), false),
    ] {
        for kind in [TOOL, ROUTED_OUTPUT] {
            assert_eq!(
                resolve(
                    &with_default,
                    &payload,
                    ReplyChoice::Default,
                    registered,
                    kind
                ),
                default_chat()
            );
            assert_eq!(
                resolve(&without, &payload, ReplyChoice::Default, registered, kind),
                unavailable
            );
        }
    }
}

#[test]
fn with_the_flag_off_a_default_request_is_rejected_never_redirected() {
    let def = channel(Some(telegram_default()), |_| {});
    let unavailable = EffectiveAddress::Rejected {
        reason: DEFAULT_REPLY_UNAVAILABLE,
    };
    let payload = event(chat_origin());
    // An automatic output whose consumer asks for `default`.
    assert!(!default_ok(&def, &payload, false, ROUTED_OUTPUT));
    let output = requested(None, ReplyChoice::Default, &payload);
    assert_eq!(
        effective(&def, &payload, output, false, false, ROUTED_OUTPUT),
        unavailable
    );
    // A hand-off whose publisher asked for `default`.
    let mut handoff = payload.clone();
    handoff.lineage = Some(Lineage {
        root: "0123456789abcdef0123456789abcdef".into(),
        hop: 1,
        path: vec![ConsumerId::parse(REVIEWER).unwrap()],
        reply: ReplyChoice::Default,
    });
    for kind in [TOOL, ROUTED_OUTPUT] {
        let asked = requested(None, ReplyChoice::Origin, &handoff);
        assert_eq!(asked, ReplyChoice::Default);
        assert_eq!(
            effective(&def, &handoff, asked, false, false, kind),
            unavailable
        );
    }
    // An incapable origin is never redirected to a default with the flag off.
    let none = event(ReplyAddress::None);
    assert_eq!(
        effective(&def, &none, ReplyChoice::Origin, false, false, TOOL),
        EffectiveAddress::Origin(ReplyAddress::None)
    );
}

#[test]
fn an_unrouted_turns_outputs_always_go_to_the_origin() {
    let unrouted = SendKind::Output { routed_turn: false };
    let def = channel(Some(telegram_default()), |_| {});
    for payload in [
        event(chat_origin()),
        event(sync_origin()),
        event(ReplyAddress::None),
    ] {
        assert!(!default_ok(&def, &payload, true, unrouted));
        for choice in [ReplyChoice::Origin, ReplyChoice::Default] {
            for registered in [false, true] {
                assert_eq!(
                    resolve(&def, &payload, choice, registered, unrouted),
                    EffectiveAddress::Origin(payload.reply.clone())
                );
            }
        }
    }
}

#[test]
fn requests_follow_the_tool_then_the_consumer_then_the_lineage_then_the_origin() {
    let mut payload = event(chat_origin());
    assert_eq!(
        requested(None, ReplyChoice::Origin, &payload),
        ReplyChoice::Origin
    );
    assert_eq!(
        requested(None, ReplyChoice::Default, &payload),
        ReplyChoice::Default
    );
    payload.lineage = Some(Lineage {
        root: "0123456789abcdef0123456789abcdef".into(),
        hop: 1,
        path: vec![ConsumerId::parse(REVIEWER).unwrap()],
        reply: ReplyChoice::Default,
    });
    assert_eq!(
        requested(None, ReplyChoice::Origin, &payload),
        ReplyChoice::Default
    );
    // A tool's explicit destination comes before the lineage.
    assert_eq!(
        requested(Some(ReplyChoice::Origin), ReplyChoice::Origin, &payload),
        ReplyChoice::Origin
    );
    payload.lineage.as_mut().unwrap().reply = ReplyChoice::Origin;
    assert_eq!(
        requested(Some(ReplyChoice::Default), ReplyChoice::Origin, &payload),
        ReplyChoice::Default
    );
    assert_eq!(
        requested(None, ReplyChoice::Default, &payload),
        ReplyChoice::Default,
        "the consumer's own destination comes first"
    );
}

#[test]
fn each_snapshot_resolves_the_default_it_names_now() {
    let payload = event(ReplyAddress::None);
    let unavailable = EffectiveAddress::Rejected {
        reason: DEFAULT_REPLY_UNAVAILABLE,
    };
    let before = channel(Some(telegram_default()), |_| {});
    let after = channel(Some(relay_default()), |_| {});
    // A send carrying the earlier snapshot keeps its destination; one
    // carrying the later snapshot takes the new one.
    assert_eq!(
        resolve(&before, &payload, ReplyChoice::Default, false, TOOL),
        default_chat()
    );
    assert!(matches!(
        resolve(&after, &payload, ReplyChoice::Default, false, TOOL),
        EffectiveAddress::DefaultRelayConversation { .. }
    ));
    // Snapshots whose default can no longer be reached resolve none.
    let detached = channel(Some(telegram_default()), |value| {
        value["sources"].as_array_mut().unwrap().remove(1);
    });
    let tokenless = channel(Some(telegram_default()), |value| {
        value["secrets"]
            .as_object_mut()
            .unwrap()
            .remove("telegram_bot_token");
    });
    let keyless = channel(Some(relay_default()), |value| {
        value["secrets"]
            .as_object_mut()
            .unwrap()
            .remove("channel_key");
    });
    let unrouted_route = channel(Some(relay_default()), |value| {
        value["sources"][2]["route_ids"] = json!(["7f1c9a3e-2b4d-4c5e-8f70-112233445566"]);
    });
    let sinkless = channel(Some(json!({"type":"sinks"})), |_| {});
    for def in [&detached, &tokenless, &keyless, &unrouted_route, &sinkless] {
        assert_eq!(resolve_now(def), None);
        assert_eq!(
            resolve(def, &payload, ReplyChoice::Default, false, TOOL),
            unavailable
        );
        assert_eq!(
            resolve(def, &payload, ReplyChoice::Origin, false, TOOL),
            EffectiveAddress::Origin(ReplyAddress::None),
            "an origin request keeps today's address"
        );
    }
}
