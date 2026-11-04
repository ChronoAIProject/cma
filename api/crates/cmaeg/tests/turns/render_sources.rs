//! Which bot address a message's input drops: on a channel with extended
//! sources the bot of the source the message arrived on, and on a legacy
//! channel the first chat source's bot, exactly as before (invariant I5).
use cmaeg::turns::{
    render_input,
    render_media::{Audience, RenderMedia},
};
use cmaeg_core::{
    activity::{Activity, ActivityKind, SourceKind},
    channel::{ChannelDefinition, Platform},
    profile::EventWorkerProfile,
};
use serde_json::{json, Value};

fn native() -> Value {
    json!({"type":"telegram","bot_id":"77","bot_username":"native_bot","admission":{"type":"all"}})
}

fn relay(platform: &str, route: &str, username: &str) -> Value {
    json!({"type":"nyxid_relay","issuer":"https://nyx.example","jwks_url":"https://nyx.example/jwks",
        "key_id":"key","route_ids":[route],"platform":platform,"bot_username":username,
        "admission":{"type":"all"}})
}

fn definition(sources: Value) -> ChannelDefinition {
    let mut value: Value = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    let profile: EventWorkerProfile = serde_json::from_value(value["profile"].clone()).unwrap();
    value["profile_digest"] = profile.digest().into();
    value["sources"] = sources;
    value["partition"]["key"] = "conversation".into();
    value["secrets"]["telegram_bot_token"] = "77:fixture".into();
    value["secrets"]["telegram_webhook_secret"] = "native-webhook-fixture".into();
    let definition: ChannelDefinition = serde_json::from_value(value).unwrap();
    definition.validate().unwrap();
    definition
}

/// The rendered text of an addressed message from `source`.
fn rendered(definition: &ChannelDefinition, source: SourceKind, text: &str) -> String {
    let mut activity: Activity = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/activity.json"
    ))
    .unwrap();
    activity.source = source;
    activity.kind = ActivityKind::Message {
        text: text.into(),
        attachments: vec![],
        mentions_bot: true,
        reply_to: None,
        quoted_message: None,
        album_id: None,
    };
    let input = render_input::render(
        &activity,
        definition,
        &RenderMedia::empty(None),
        Audience {
            tools: false,
            downloads: false,
        },
    );
    input[0]["content"][0]["text"].as_str().unwrap().to_owned()
}

fn relayed() -> SourceKind {
    SourceKind::NyxidRelay {
        platform: Platform::Telegram,
        route_id: "route-telegram".into(),
    }
}

fn natively() -> SourceKind {
    SourceKind::Telegram {
        platform: Platform::Telegram,
        bot_id: "77".into(),
    }
}

#[test]
fn extended_channels_strip_the_bot_of_the_activitys_own_source() {
    // The first chat source is a Lark relay, whose username names no
    // Telegram bot; each Telegram activity drops only its own bot.
    let extended = definition(json!([
        relay("lark", "route-lark", "lark_bot"),
        native(),
        relay("telegram", "route-telegram", "relay_bot"),
    ]));
    assert!(extended.has_extended_sources());
    assert_eq!(
        rendered(&extended, relayed(), "@relay_bot hi @native_bot"),
        "hi @native_bot"
    );
    assert_eq!(
        rendered(&extended, natively(), "@native_bot hi @relay_bot"),
        "hi @relay_bot"
    );
    // An activity whose source is no longer configured strips nothing.
    let foreign = SourceKind::Telegram {
        platform: Platform::Telegram,
        bot_id: "78".into(),
    };
    assert_eq!(
        rendered(&extended, foreign, "@native_bot hi"),
        "@native_bot hi"
    );
}

#[test]
fn legacy_channels_strip_the_first_chat_sources_bot() {
    let legacy = definition(json!([
        native(),
        relay("telegram", "route-telegram", "relay_bot")
    ]));
    assert!(!legacy.has_extended_sources());
    // Unchanged: the relay activity drops the native bot's address, not its own.
    assert_eq!(
        rendered(&legacy, relayed(), "@relay_bot hi @native_bot"),
        "@relay_bot hi"
    );
    assert_eq!(
        rendered(&legacy, natively(), "@native_bot hi @relay_bot"),
        "hi @relay_bot"
    );
}
