use cmaeg_core::{
    activity::SourceKind,
    channel::{ChannelDefinition, Platform, Source},
    profile::EventWorkerProfile,
};
use serde_json::{json, Value};

fn relay(platform: &str, route: &str, username: &str) -> Value {
    json!({"type":"nyxid_relay","issuer":"https://nyx.example",
        "jwks_url":"https://nyx.example/.well-known/jwks.json","key_id":"key",
        "route_ids":[route, format!("{route}-b")],"platform":platform,
        "bot_username":username,"admission":{"type":"all"}})
}

fn definition(sources: Value, custom_key: Option<&str>) -> ChannelDefinition {
    let mut value: Value = serde_json::from_str(include_str!("fixtures/channel.json")).unwrap();
    let profile: EventWorkerProfile = serde_json::from_value(value["profile"].clone()).unwrap();
    value["profile_digest"] = json!(profile.digest());
    value["sources"] = sources;
    value["partition"]["key"] = json!("conversation");
    if let Some(key) = custom_key {
        value["partition"]["custom_key"] = json!(key);
    }
    value["secrets"]["telegram_bot_token"] = json!("77:fixture");
    value["secrets"]["telegram_webhook_secret"] = json!("native-webhook-fixture");
    let definition: ChannelDefinition = serde_json::from_value(value).unwrap();
    definition.validate().unwrap();
    definition
}

fn extended() -> ChannelDefinition {
    definition(
        json!([
            {"type":"telegram","bot_id":"77","bot_username":"native_bot","admission":{"type":"all"}},
            relay("lark", "route-lark", "lark_bot"),
            relay("telegram", "route-telegram", "relay_bot"),
            {"type":"custom","auth":{"type":"hmac","key_id":"key-1"}},
        ]),
        Some("event_subject"),
    )
}

fn native(bot_id: &str) -> SourceKind {
    SourceKind::Telegram {
        platform: Platform::Telegram,
        bot_id: bot_id.into(),
    }
}

fn relayed(platform: Platform) -> SourceKind {
    SourceKind::NyxidRelay {
        platform,
        route_id: "any-route".into(),
    }
}

#[test]
fn source_of_finds_the_activitys_own_source_for_every_variant() {
    let definition = extended();
    for (kind, expected) in [
        (native("77"), Some("telegram")),
        (native("78"), None),
        (relayed(Platform::Lark), Some("relay_lark")),
        (relayed(Platform::Telegram), Some("relay_telegram")),
        (relayed(Platform::Feishu), None),
        (SourceKind::Custom, Some("custom")),
    ] {
        assert_eq!(
            definition
                .source_of(&kind)
                .map(|source| source.source_id().as_str().to_owned())
                .as_deref(),
            expected,
            "{kind:?}"
        );
    }
    let relay_only = definition_with_relay();
    assert!(relay_only.source_of(&SourceKind::Custom).is_none());
    assert!(relay_only.source_of(&native("77")).is_none());
}

fn definition_with_relay() -> ChannelDefinition {
    definition(json!([relay("lark", "route-lark", "lark_bot")]), None)
}

#[test]
fn a_route_selects_the_one_relay_that_owns_it() {
    let definition = extended();
    for (route, platform) in [
        ("route-lark", Some(Platform::Lark)),
        ("route-lark-b", Some(Platform::Lark)),
        ("route-telegram", Some(Platform::Telegram)),
        ("route-telegram-b", Some(Platform::Telegram)),
        ("route-unknown", None),
        ("", None),
    ] {
        let found = definition
            .relay_source_for_route(route)
            .map(|source| match source {
                Source::NyxidRelay { platform, .. } => *platform,
                other => panic!("a route selected a non-relay source: {other:?}"),
            });
        assert_eq!(found, platform, "{route}");
    }
}

#[test]
fn bot_usernames_come_from_the_activitys_own_source() {
    let definition = extended();
    assert_eq!(
        definition.bot_username_of(&native("77")),
        Some("native_bot")
    );
    assert_eq!(
        definition.bot_username_of(&relayed(Platform::Telegram)),
        Some("relay_bot")
    );
    assert_eq!(
        definition.bot_username_of(&relayed(Platform::Lark)),
        Some("lark_bot")
    );
    assert_eq!(definition.bot_username_of(&native("78")), None);
    assert_eq!(definition.bot_username_of(&SourceKind::Custom), None);
}

#[test]
fn extended_sources_are_told_apart_from_legacy_ones() {
    assert!(extended().has_extended_sources());
    assert!(!definition_with_relay().has_extended_sources());
    // A custom key alone needs the extended readers.
    assert!(definition(
        json!([relay("lark", "route-lark", "lark_bot")]),
        Some("fixed")
    )
    .has_extended_sources());
}
