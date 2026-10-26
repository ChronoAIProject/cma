//! Pinned fixtures of the legacy delivery recording: one master key, fixed
//! channel ids and credentials, so every sealed or derived value but the
//! random nonces repeats across runs.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg::{channels::resolver::ResolvedChannel, ingress::launch::LaunchRequest};
use cmaeg_core::{
    activity::{Activity, SourceKind},
    channel::{ChannelDefinition, Platform, Secret},
    ids::{ChannelId, IdempotencyKey},
    seal::{
        event_ref::activity_digest, ChannelPointer, EventRef, EventRefPayload, MasterKeyRing,
        ReplyAddress,
    },
};
use serde_json::{json, Value};
use std::sync::Arc;

pub const REPLY_TOKEN: &str = "recording-reply-token";
pub const CHANNEL_KEY: &str = "recording-channel-key";
pub const BOT_TOKEN: &str = "77:recording";
pub const WEBHOOK_URL: &str = "https://receiver.example/events";
const PROVIDER_BEARER: &str = "recording-provider-bearer";
/// A token expiry no recording reaches: 2100-01-01.
const FAR: i64 = 4_102_444_800;

/// The pinned master key ring every recording seals with.
pub fn keys() -> Arc<MasterKeyRing> {
    Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([7; 32]))).unwrap())
}

/// The fixture channel under the pinned `id`, after `edit`.
fn channel(id: &str, edit: impl FnOnce(&mut Value)) -> Arc<ResolvedChannel> {
    let mut value: Value = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    value["secrets"]["provider_credential"] = PROVIDER_BEARER.into();
    edit(&mut value);
    let definition: ChannelDefinition = serde_json::from_value(value).unwrap();
    Arc::new(ResolvedChannel {
        id: ChannelId::new(id).unwrap(),
        pointer: ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: "https://cma.example/api/v1/gateway/channel-records/record-1".into(),
        },
        definition,
        agent_key_hash: [0; 32],
        consumer_keys: Vec::new(),
        loaded_at: std::time::Instant::now(),
        version: 1,
    })
}

/// A NyxID relay channel on Telegram with a channel key.
pub fn relay_channel(ack_after_ms: u32) -> Arc<ResolvedChannel> {
    channel("cmaeg1.ch.recording-relay", |value| {
        value["sources"] = json!([{"type":"nyxid_relay","issuer":"https://nyx.example","jwks_url":"https://nyx.example/jwks","key_id":"key-1","route_ids":["route-1"],"platform":"telegram","admission":{"type":"all"}}]);
        value["reply"]["ack_after_ms"] = ack_after_ms.into();
        value["secrets"]["channel_key"] = CHANNEL_KEY.into();
    })
}

/// A custom channel answering through Sync and one result webhook.
pub fn custom_channel() -> Arc<ResolvedChannel> {
    channel("cmaeg1.ch.recording-custom", |value| {
        value["reply"]["ack_after_ms"] = 0.into();
        value["reply"]["sinks"] = json!([{"type":"sync"},{"type":"result_webhook","url":WEBHOOK_URL,"hmac_key_id":"result"}]);
        value["secrets"]["hmac_secrets"] = json!([["result", "recording-webhook-key"]]);
    })
}

/// A native Telegram channel for bot 77.
pub fn native_channel() -> Arc<ResolvedChannel> {
    channel("cmaeg1.ch.recording-native", |value| {
        value["sources"] = json!([{"type":"telegram","bot_id":"77","bot_username":"fixture_bot","admission":{"type":"all"}}]);
        value["reply"]["ack_after_ms"] = 0.into();
        value["secrets"]["telegram_bot_token"] = BOT_TOKEN.into();
    })
}

/// The fixture activity as event `event_id`, received from `source`.
pub fn activity(source: SourceKind, event_id: &str) -> Activity {
    let mut activity: Activity = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/activity.json"
    ))
    .unwrap();
    activity.event_id = event_id.parse().unwrap();
    activity.source = source;
    activity
}

pub fn relay_activity() -> Activity {
    activity(
        SourceKind::NyxidRelay {
            platform: Platform::Telegram,
            route_id: "route-1".into(),
        },
        "msg-1",
    )
}

pub fn relay_reply() -> ReplyAddress {
    ReplyAddress::NyxidRelay {
        thread_reply: false,
        message_id: "inbound-1".into(),
        route_id: "route-1".into(),
        platform: Platform::Telegram,
        reply_token: Secret::new(REPLY_TOKEN),
        reply_token_expires_at: FAR,
    }
}

/// The event `activity` opened on `channel`, answered at `reply`, in the
/// well-formed provider conversation numbered `conversation`.
pub fn event(
    channel: &ResolvedChannel,
    activity: &Activity,
    conversation: u8,
    reply: ReplyAddress,
) -> EventRefPayload {
    EventRefPayload {
        v: 1,
        activity_sha256: Some(activity_digest(activity)),
        channel_id: channel.id.clone(),
        record_id: channel.pointer.record_id.clone(),
        event_id: activity.event_id.clone(),
        partition_key: activity.event_id.as_str().into(),
        conversation_id: format!("conv_{conversation:032x}").parse().unwrap(),
        expires_at: FAR,
        reply,
        consumer: None,
        lineage: None,
    }
}

/// The admitted turn whose outcomes the engine delivers for `event`.
pub fn turn(
    keys: &MasterKeyRing,
    channel: &Arc<ResolvedChannel>,
    activity: Activity,
    event: &EventRefPayload,
) -> Arc<LaunchRequest> {
    Arc::new(LaunchRequest {
        timing: None,
        consumer: None,
        trace: tracing::Span::none(),
        channel: channel.clone(),
        idempotency_key: IdempotencyKey::from_event(&activity.event_id),
        conversation_id: event.conversation_id.clone(),
        event_ref: EventRef::seal(event, keys).unwrap(),
        received_at: std::time::Instant::now(),
        activity,
    })
}
