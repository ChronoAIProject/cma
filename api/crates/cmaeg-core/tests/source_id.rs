use cmaeg_core::{
    activity::SourceKind,
    channel::{Platform, Source, SourceId, AGENT_SOURCE_ID},
};
use serde_json::json;

const VALID: [&str; 10] = [
    "telegram",
    "discord",
    "relay_telegram",
    "relay_lark",
    "relay_feishu",
    "relay_discord",
    "relay_slack",
    "relay_whatsapp",
    "custom",
    "agent",
];

const PLATFORMS: [(Platform, &str); 6] = [
    (Platform::Telegram, "relay_telegram"),
    (Platform::Lark, "relay_lark"),
    (Platform::Feishu, "relay_feishu"),
    (Platform::Discord, "relay_discord"),
    (Platform::Slack, "relay_slack"),
    (Platform::Whatsapp, "relay_whatsapp"),
];

fn relay(platform: Platform, route_ids: &[&str]) -> Source {
    serde_json::from_value(json!({"type":"nyxid_relay","issuer":"https://nyx.example",
        "jwks_url":"https://nyx.example/.well-known/jwks.json","key_id":"key",
        "route_ids":route_ids,"platform":platform,"admission":{"type":"all"}}))
    .unwrap()
}

#[test]
fn the_grammar_accepts_exactly_the_ten_slot_ids() {
    for value in VALID {
        let id = SourceId::parse(value).unwrap();
        assert_eq!(id.as_str(), value);
        assert_eq!(id.is_agent(), value == AGENT_SOURCE_ID);
        assert_eq!(serde_json::to_value(&id).unwrap(), json!(value));
        assert_eq!(
            serde_json::from_value::<SourceId>(json!(value)).unwrap(),
            id
        );
    }
}

#[test]
fn the_grammar_rejects_everything_else() {
    for value in [
        "",
        "relay_",
        "relay_teams",
        "relay_telegram-new",
        "Telegram",
        "RELAY_LARK",
        "agent2",
        " custom",
        "lark",
    ] {
        let error = SourceId::parse(value).unwrap_err();
        assert_eq!(error.pointer, "/source_id", "{value:?}");
        assert!(
            serde_json::from_value::<SourceId>(json!(value)).is_err(),
            "{value:?}"
        );
    }
}

#[test]
fn configured_sources_and_activity_sources_derive_the_same_slot() {
    let telegram: Source =
        serde_json::from_value(json!({"type":"telegram","bot_id":"77","admission":{"type":"all"}}))
            .unwrap();
    let custom: Source =
        serde_json::from_value(json!({"type":"custom","auth":{"type":"hmac","key_id":"k"}}))
            .unwrap();
    assert_eq!(telegram.source_id().as_str(), "telegram");
    assert_eq!(custom.source_id().as_str(), "custom");
    let native = SourceKind::Telegram {
        platform: Platform::Telegram,
        bot_id: "77".into(),
    };
    assert_eq!(native.source_id().as_str(), "telegram");
    assert_eq!(SourceKind::Custom.source_id().as_str(), "custom");
    let discord: Source = serde_json::from_value(
        json!({"type":"discord","bot_id":"77","intents":37377,"admission":{"type":"all"}}),
    )
    .unwrap();
    assert_eq!(
        discord.source_id(),
        SourceKind::Discord {
            bot_id: "77".into()
        }
        .source_id()
    );
    for (platform, expected) in PLATFORMS {
        assert_eq!(relay(platform, &["route-1"]).source_id().as_str(), expected);
        let activity = SourceKind::NyxidRelay {
            platform,
            route_id: "route-1".into(),
        };
        assert_eq!(activity.source_id().as_str(), expected);
        assert!(!activity.source_id().is_agent());
    }
}

#[test]
fn a_re_created_relay_keeps_its_slot_id() {
    // Detaching a relay and attaching a new one on the same platform, even
    // with other routes, names the same slot.
    let before = relay(Platform::Lark, &["route-1"]).source_id();
    let after = relay(Platform::Lark, &["route-9", "route-10"]).source_id();
    assert_eq!(before, after);
    assert_ne!(before, relay(Platform::Feishu, &["route-1"]).source_id());
}
