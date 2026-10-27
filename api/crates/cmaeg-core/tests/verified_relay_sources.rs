use cmaeg_core::{
    channel::{ChannelDefinition, Source},
    profile::EventWorkerProfile,
};
use serde_json::{json, Value};

fn source(platform: &str) -> Value {
    json!({"type":"nyxid_relay","issuer":"https://identity.example","jwks_url":"https://identity.example/jwks","key_id":"key","route_ids":["route"],"platform":platform,"admission":{"type":"all"}})
}

fn definition(source: Value) -> ChannelDefinition {
    let mut value: Value = serde_json::from_str(include_str!("fixtures/channel.json")).unwrap();
    let profile: EventWorkerProfile = serde_json::from_value(value["profile"].clone()).unwrap();
    value["profile_digest"] = json!(profile.digest());
    value["sources"] = json!([source]);
    value["partition"]["key"] = json!("conversation_and_sender");
    serde_json::from_value(value).unwrap()
}

#[test]
fn retained_normalized_sources_keep_their_exact_serialized_shape() {
    for platform in ["discord", "whatsapp", "slack"] {
        for trust in [false, true] {
            let mut old = source(platform);
            if trust {
                old["trust_normalized"] = true.into();
            }
            let parsed: Source = serde_json::from_value(old.clone()).unwrap();
            assert_eq!(serde_json::to_value(&parsed).unwrap(), old);
            definition(old).validate().unwrap();
            let mut explicit = serde_json::to_value(parsed).unwrap();
            explicit["verify_raw_event"] = false.into();
            let explicit: Source = serde_json::from_value(explicit).unwrap();
            assert!(serde_json::to_value(explicit)
                .unwrap()
                .get("verify_raw_event")
                .is_none());
        }
    }
}

#[test]
fn verified_mode_is_explicit_platform_limited_and_never_combined_with_trust() {
    for platform in ["telegram", "lark", "feishu", "discord", "whatsapp", "slack"] {
        for trust in [false, true] {
            let mut value = source(platform);
            value["verify_raw_event"] = true.into();
            value["trust_normalized"] = trust.into();
            let result = definition(value).validate();
            if matches!(platform, "discord" | "whatsapp") && !trust {
                result.unwrap();
            } else {
                assert_eq!(result.unwrap_err().pointer, "/sources/0/verify_raw_event");
            }
        }
    }
}

#[test]
fn verified_discord_receiving_identity_must_be_a_numeric_bot_id() {
    let mut value = source("discord");
    value["bot_id"] = "legacy-bot-label".into();
    value["trust_normalized"] = true.into();
    definition(value.clone()).validate().unwrap();
    value["trust_normalized"] = false.into();
    value["verify_raw_event"] = true.into();
    assert_eq!(
        definition(value.clone()).validate().unwrap_err().pointer,
        "/sources/0/bot_id"
    );
    value["bot_id"] = "1234567890".into();
    definition(value).validate().unwrap();
}
