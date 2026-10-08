//! A routed channel with a default reply source, and custom events of it
//! whose origin cannot take a reply, on the real engine.
use super::{
    default_relay::ROUTE,
    fake_http::{FakeHttp, Sent},
    group_fixtures::{consumer_event, consumer_turn, routed_custom_activity},
    recording_fixtures::{BOT_TOKEN, WEBHOOK_URL},
    recording_support::{Hooks, Recording},
};
use cmaeg::{
    channels::resolver::ResolvedChannel, delivery::intents::ReplySource,
    turns::outcomes::TurnOutcome, DeliveryConfig,
};
use cmaeg_core::{
    ids::ConsumerId,
    seal::{EventRefPayload, ReplyAddress},
};
use serde_json::{json, Value};
use std::sync::Arc;

/// The default chat: a long id, so a log holding it in full is visible.
pub const CHAT: &str = "-1001234567890";

pub fn telegram_default() -> Value {
    json!({"type":"telegram_chat","source_id":"telegram","chat_id":CHAT,"thread_id":"7"})
}

pub fn relay_default() -> Value {
    json!({"type":"relay_conversation","source_id":"relay_telegram","route_id":ROUTE})
}

pub fn sinks_default() -> Value {
    json!({"type":"sinks"})
}

/// The routed channel of the group fixtures with a custom source, a native
/// bot and a relay conversation, defaulting to `default_reply`, after `edit`.
pub fn default_channel(
    default_reply: Value,
    edit: impl FnOnce(&mut Value),
) -> Arc<ResolvedChannel> {
    super::group_fixtures::routed_channel(1000, |value| {
        value["sources"] = json!([
            {"type":"custom","auth":{"type":"hmac","key_id":"key-1"}},
            {"type":"telegram","bot_id":"77","bot_username":"fixture_bot","admission":{"type":"all"}},
            {"type":"nyxid_relay","issuer":"https://nyx.example","jwks_url":"https://nyx.example/jwks",
                "key_id":"key-1","route_ids":[ROUTE],"platform":"telegram","admission":{"type":"all"}}
        ]);
        value["secrets"]["telegram_bot_token"] = BOT_TOKEN.into();
        value["routing"]["default_reply"] = default_reply;
        edit(value);
    })
}

/// Add the result webhook sink.
pub fn with_sinks(value: &mut Value) {
    value["reply"]["sinks"] =
        json!([{"type":"result_webhook","url":WEBHOOK_URL,"hmac_key_id":"result"}]);
    value["secrets"]["hmac_secrets"] = json!([["result", "recording-webhook-key"]]);
}

/// The routed custom event as `consumer` receives it, answered at `reply`.
pub fn custom_event(
    channel: &ResolvedChannel,
    consumer: Option<ConsumerId>,
    reply: ReplyAddress,
) -> EventRefPayload {
    EventRefPayload {
        reply,
        ..consumer_event(channel, &routed_custom_activity(), consumer)
    }
}

/// A recording of the platforms, with the multi-consumer flag at `flag`.
pub fn recording(http: FakeHttp, flag: bool) -> Recording {
    let config = DeliveryConfig {
        multi_consumer_enabled: flag,
        ..Default::default()
    };
    Recording::start(http, Hooks::answering([]), config)
}

/// Run `event`'s consumer's routed custom turn to `outcomes`, in order.
pub async fn run(
    recording: &Recording,
    channel: &Arc<ResolvedChannel>,
    event: &EventRefPayload,
    outcomes: impl IntoIterator<Item = TurnOutcome>,
) {
    let turn = consumer_turn(&recording.keys, channel, routed_custom_activity(), event);
    for outcome in outcomes {
        recording.outcome(&turn, outcome).await;
    }
}

pub fn completed(text: &str) -> TurnOutcome {
    TurnOutcome::Completed {
        final_text: Some(text.into()),
    }
}

/// Reply to `event` with `message` as a tool does.
pub async fn tool(
    recording: &mut Recording,
    channel: &Arc<ResolvedChannel>,
    event: &EventRefPayload,
    message: Value,
) -> cmaeg::delivery::intents::IntentReceipt {
    recording
        .reply(channel, event, message, ReplySource::Tool)
        .await
}

/// The requests sent to NyxID's proactive send.
pub fn initiated(sent: &[Sent]) -> Vec<&Sent> {
    sent.iter()
        .filter(|sent| sent.url.path() == "/api/v1/channel-relay/send")
        .collect()
}

/// The Bot API messages sent, as (chat, thread, text).
pub fn telegram_messages(sent: &[Sent]) -> Vec<(String, Option<String>, String)> {
    sent.iter()
        .filter(|sent| sent.url.path().ends_with("/sendMessage"))
        .map(|sent| {
            let body = sent.json();
            (
                body["chat_id"].as_str().unwrap().to_owned(),
                body["message_thread_id"].as_str().map(str::to_owned),
                body["text"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}
