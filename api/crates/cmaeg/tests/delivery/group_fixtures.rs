//! A routed relay channel with two consumers, `lead` (the default consumer)
//! and `reviewer`, and the turns each runs for one relay event.
use super::recording_fixtures::{
    activity, channel, event, relay_activity, relay_reply, CHANNEL_KEY, WEBHOOK_URL,
};
use cmaeg::{channels::resolver::ResolvedChannel, ingress::launch::LaunchRequest};
use cmaeg_core::{
    activity::{Activity, ActivityRouting, ConsumerTarget, RouteResolution, SourceKind},
    channel::{ReplyChoice, SourceId},
    ids::{ConsumerId, ConsumerName, IdempotencyKey},
    seal::{EventRef, EventRefPayload, Lineage, MasterKeyRing},
};
use serde_json::{json, Value};
use std::sync::Arc;

pub const REVIEWER: &str = "csm_00000000000a";
/// A consumer that publishes hand-offs; it never answers here.
const PUBLISHER: &str = "csm_00000000000b";

pub fn reviewer() -> ConsumerId {
    ConsumerId::parse(REVIEWER).unwrap()
}

/// A Telegram relay channel routing every event to `lead` and `reviewer`,
/// acknowledging after `ack_after_ms`, after `edit`.
pub fn routed_channel(ack_after_ms: u32, edit: impl FnOnce(&mut Value)) -> Arc<ResolvedChannel> {
    channel("cmaeg1.ch.recording-routed", |value| {
        value["sources"] = json!([{"type":"nyxid_relay","issuer":"https://nyx.example","jwks_url":"https://nyx.example/jwks","key_id":"key-1","route_ids":["route-1"],"platform":"telegram","admission":{"type":"all"}}]);
        value["reply"]["ack_after_ms"] = ack_after_ms.into();
        value["secrets"]["channel_key"] = CHANNEL_KEY.into();
        let reviewer = json!({"id": REVIEWER, "name": "reviewer",
            "profile": value["profile"], "profile_digest": value["profile_digest"],
            "provider": value["provider"], "agent_key_hash": value["secrets"]["agent_key_hash"],
            "created_at": "2026-10-01T00:00:00Z"});
        value["routing"] = json!({"version": 1, "default_consumer": {"name": "lead"},
            "consumers": [reviewer], "default_targets": ["default", REVIEWER]});
        edit(value);
    })
}

/// The routed channel moved to a custom source answering through one
/// result webhook.
pub fn routed_custom_channel() -> Arc<ResolvedChannel> {
    routed_channel(0, |value| {
        value["sources"] = json!([{"type":"custom","auth":{"type":"hmac","key_id":"key-1"}}]);
        value["reply"]["sinks"] =
            json!([{"type":"result_webhook","url":WEBHOOK_URL,"hmac_key_id":"result"}]);
        value["secrets"]["hmac_secrets"] = json!([["result", "recording-webhook-key"]]);
    })
}

/// A custom event, routed by default to both consumers.
pub fn routed_custom_activity() -> Activity {
    let mut custom = activity(SourceKind::Custom, "msg-1");
    let mut routing = routed_activity().routing.unwrap();
    routing.source_id = SourceId::parse("custom").unwrap();
    custom.routing = Some(routing);
    custom
}

/// The relay activity, routed by default to both consumers.
pub fn routed_activity() -> Activity {
    let mut activity = relay_activity();
    activity.routing = Some(ActivityRouting {
        source_id: SourceId::parse("relay_telegram").unwrap(),
        resolution: RouteResolution::Default,
        rule_id: None,
        targets: vec![target("default", "lead"), target(REVIEWER, "reviewer")],
        addressing_bytes: 0,
        causation: None,
    });
    activity
}

fn target(id: &str, name: &str) -> ConsumerTarget {
    ConsumerTarget {
        id: ConsumerId::parse(id).unwrap(),
        name: ConsumerName::parse(name).unwrap(),
    }
}

/// The relay event as `consumer` receives it; `None` is `lead`.
pub fn consumer_event(
    channel: &ResolvedChannel,
    activity: &Activity,
    consumer: Option<ConsumerId>,
) -> EventRefPayload {
    let conversation = if consumer.is_some() { 2 } else { 1 };
    EventRefPayload {
        consumer,
        ..event(channel, activity, conversation, relay_reply())
    }
}

/// `event` continued as a hand-off one hop from another consumer.
pub fn handoff(mut event: EventRefPayload) -> EventRefPayload {
    event.lineage = Some(Lineage {
        root: "0123456789abcdef0123456789abcdef".into(),
        hop: 1,
        path: vec![ConsumerId::parse(PUBLISHER).unwrap()],
        reply: ReplyChoice::Origin,
    });
    event
}

/// The turn `event`'s consumer runs for `activity`.
pub fn consumer_turn(
    keys: &MasterKeyRing,
    channel: &Arc<ResolvedChannel>,
    activity: Activity,
    event: &EventRefPayload,
) -> Arc<LaunchRequest> {
    Arc::new(LaunchRequest {
        sync_fanout: false,
        lease: std::sync::Mutex::new(None),
        timing: None,
        consumer: event.consumer.clone(),
        trace: tracing::Span::none(),
        channel: channel.clone(),
        idempotency_key: IdempotencyKey::from_event(&activity.event_id),
        conversation_id: event.conversation_id.clone(),
        event_ref: EventRef::seal(event, keys).unwrap(),
        received_at: std::time::Instant::now(),
        activity,
    })
}
