use cmaeg_core::{channel::ChannelDefinition, profile::EventWorkerProfile};
use serde_json::{json, Value};

#[path = "channel_validation/extended.rs"]
mod extended;
#[path = "channel_validation/legacy.rs"]
mod legacy;

fn channel() -> Value {
    let mut value: Value = serde_json::from_str(include_str!("fixtures/channel.json")).unwrap();
    let profile: EventWorkerProfile = serde_json::from_value(value["profile"].clone()).unwrap();
    value["profile_digest"] = json!(profile.digest());
    value
}

fn relay() -> Value {
    json!({"type":"nyxid_relay","issuer":"https://identity.example","jwks_url":"https://identity.example/jwks","key_id":"key","route_ids":["route"],"platform":"telegram","admission":{"type":"all"}})
}

fn relay_channel() -> Value {
    let mut value = channel();
    value["sources"] = json!([relay()]);
    value["partition"]["key"] = json!("conversation_and_sender");
    value
}

/// Validate against the legacy rules, which these tables pin even once the
/// reader accepts more.
fn validate(value: Value) -> Result<(), cmaeg_core::ValidationError> {
    let definition = serde_json::from_value::<ChannelDefinition>(value).unwrap();
    let legacy = definition.validate_legacy();
    // The reader differs from the legacy rules only where they refuse the
    // sources or partition, or `custom_key` asks for the extended rules;
    // everywhere else it answers exactly as they do.
    if definition.validate_legacy_sources_and_partition().is_ok()
        && definition.partition.custom_key.is_none()
    {
        assert_eq!(definition.validate(), legacy);
    }
    legacy
}

#[test]
fn rejects_schema_profile_digest_and_timestamp_mismatch() {
    for (pointer, replacement) in [
        ("/schema_version", json!(2)),
        ("/profile/schema_version", json!(2)),
        (
            "/profile_digest",
            json!("sha256:".to_owned() + &"a".repeat(64)),
        ),
        ("/created_at", json!("not a timestamp")),
    ] {
        let mut value = channel();
        *value.pointer_mut(pointer).unwrap() = replacement;
        assert_eq!(validate(value).unwrap_err().pointer, pointer);
    }
}

#[test]
fn rejects_channel_source_count_and_duplicate_variants() {
    for sources in [vec![], vec![relay(); 5]] {
        let mut value = channel();
        value["sources"] = json!(sources);
        assert_eq!(validate(value).unwrap_err().pointer, "/sources");
    }
    for source in [relay(), channel()["sources"][0].clone()] {
        let mut value = channel();
        value["sources"] = json!([source.clone(), source]);
        assert_eq!(validate(value).unwrap_err().pointer, "/sources/1");
    }
}

#[test]
fn relay_routes_allow_unbound_creation_but_reject_duplicate_or_excessive_routes() {
    let mut value = relay_channel();
    for routes in [vec![], (0..64).map(|i| format!("route-{i}")).collect()] {
        value["sources"][0]["route_ids"] = json!(routes);
        validate(value.clone()).unwrap();
    }
    for routes in [
        vec!["same".to_owned(); 2],
        (0..65).map(|i| format!("route-{i}")).collect(),
    ] {
        value["sources"][0]["route_ids"] = json!(routes);
        assert_eq!(
            validate(value.clone()).unwrap_err().pointer,
            "/sources/0/route_ids"
        );
    }
    for id in [String::new(), "x".repeat(513)] {
        value["sources"][0]["route_ids"] = json!([id]);
        assert_eq!(
            validate(value.clone()).unwrap_err().pointer,
            "/sources/0/route_ids/0"
        );
    }
}

#[test]
fn rejects_empty_duplicate_and_excessive_admission_lists() {
    for (admission, pointer, list_key) in [
        (
            json!({"type":"allowlist","sender_ids":["user"]}),
            "/sources/0/admission",
            "/sender_ids",
        ),
        (
            json!({"type":"scoped","senders":{"type":"allowlist","ids":["user"]},"chats":{"type":"open"},"groups":"deny"}),
            "/sources/0/admission/senders",
            "/senders/ids",
        ),
        (
            json!({"type":"scoped","senders":{"type":"deny"},"chats":{"type":"allowlist","ids":["chat"]},"groups":"mention_or_reply_to_bot"}),
            "/sources/0/admission/chats",
            "/chats/ids",
        ),
    ] {
        let mut value = relay_channel();
        value["sources"][0]["admission"] = admission.clone();
        validate(value.clone()).unwrap();
        for ids in [
            vec![],
            vec!["same".to_owned(); 2],
            (0..513).map(|i| i.to_string()).collect(),
        ] {
            *value["sources"][0]["admission"]
                .pointer_mut(list_key)
                .unwrap() = json!(ids);
            assert_eq!(validate(value.clone()).unwrap_err().pointer, pointer);
        }
    }
}

#[test]
fn rejects_custom_auth_hashes_reserved_schema_and_template_overflow() {
    let mut value = channel();
    for hash in [
        "sha256:".to_owned() + &"A".repeat(64),
        "sha256:".to_owned() + &"g".repeat(64),
        "invalid".into(),
    ] {
        value["sources"][0]["auth"] = json!({"type":"nyxid_key","key_hashes":[hash]});
        assert_eq!(
            validate(value.clone()).unwrap_err().pointer,
            "/sources/0/auth/key_hashes"
        );
    }
    value["sources"][0]["auth"] =
        json!({"type":"nyxid_key","key_hashes":["sha256:".to_owned()+&"a".repeat(64)]});
    value["sources"][0]["prompt_template"] = json!("x".repeat(8 * 1024));
    validate(value.clone()).unwrap();
    value["sources"][0]["prompt_template"] = json!("x".repeat(8 * 1024 + 1));
    assert_eq!(
        validate(value.clone()).unwrap_err().pointer,
        "/sources/0/prompt_template"
    );
    value["sources"][0]["prompt_template"] = json!("");
    value["sources"][0]["schema"] = json!({});
    assert_eq!(validate(value).unwrap_err().pointer, "/sources/0/schema");
}

#[test]
fn partition_rules_refuse_sources_they_cannot_interpret() {
    for (mut value, supported) in [
        (channel(), vec!["fixed", "event_subject", "per_event"]),
        (
            relay_channel(),
            vec!["fixed", "conversation", "conversation_and_sender", "thread"],
        ),
    ] {
        for key in [
            "fixed",
            "event_subject",
            "per_event",
            "conversation",
            "conversation_and_sender",
            "thread",
        ] {
            value["partition"]["key"] = json!(key);
            if supported.contains(&key) {
                validate(value.clone()).unwrap();
            } else {
                assert_eq!(
                    validate(value.clone()).unwrap_err().pointer,
                    "/partition/key"
                );
            }
        }
        value["partition"]["version"] = json!(2);
        assert_eq!(validate(value).unwrap_err().pointer, "/partition/version");
    }
}

#[test]
fn channel_and_webhook_string_limits_report_exact_fields() {
    for (base, pointer, min, max) in [
        (relay_channel(), "/sources/0/issuer", 1, 512),
        (relay_channel(), "/sources/0/key_id", 1, 512),
        (channel(), "/sources/0/auth/key_id", 1, 512),
        (channel(), "/owner/subject", 1, 512),
    ] {
        let mut value = base;
        *value.pointer_mut(pointer).unwrap() = json!("x".repeat(max));
        validate(value.clone()).unwrap();
        for size in [min - 1, max + 1] {
            *value.pointer_mut(pointer).unwrap() = json!("x".repeat(size));
            assert_eq!(validate(value.clone()).unwrap_err().pointer, pointer);
        }
    }
    let mut value = channel();
    value["name"] = json!("x".repeat(80));
    validate(value.clone()).unwrap();
    value["name"] = json!("x".repeat(81));
    assert_eq!(validate(value).unwrap_err().pointer, "/name");
    let mut value = channel();
    value["reply"]["sinks"] = json!([{"type":"source"},{"type":"sync"},{"type":"result_webhook","url":"https://callback.example","hmac_key_id":"key"}]);
    validate(value.clone()).unwrap();
    value["reply"]["sinks"][2]["hmac_key_id"] = json!("");
    assert_eq!(
        validate(value.clone()).unwrap_err().pointer,
        "/reply/sinks/2/hmac_key_id"
    );
    value["reply"]["sinks"] = json!(vec![json!({"type":"sync"}); 4]);
    assert_eq!(validate(value).unwrap_err().pointer, "/reply/sinks");
}

#[test]
fn https_and_all_documented_loopback_hosts_are_accepted() {
    for url in [
        "https://example.test/path",
        "http://localhost:8080/path",
        "http://127.0.0.1/path",
        "http://[::1]:8080/path",
    ] {
        let mut value = relay_channel();
        value["sources"][0]["jwks_url"] = json!(url);
        value["reply"]["sinks"] = json!([{"type":"result_webhook","url":url,"hmac_key_id":"key"}]);
        validate(value).unwrap_or_else(|error| panic!("{url}: {error}"));
    }
}

#[test]
fn rejects_remote_http_and_missing_url_hosts() {
    for url in [
        "http://example.test/path",
        "/relative",
        "https://",
        "https://?query",
        "https://example.test/space here",
        "http://[::1]evil/path",
        "http://[2001:db8::1]/path",
        "http://localhost@example.test/path",
        "http://example.test\\@localhost/path",
        "https://example.test:invalid/path",
    ] {
        let mut value = relay_channel();
        value["sources"][0]["jwks_url"] = json!(url);
        assert_eq!(validate(value).unwrap_err().pointer, "/sources/0/jwks_url");
        let mut value = channel();
        value["reply"]["sinks"] = json!([{"type":"result_webhook","url":url,"hmac_key_id":"key"}]);
        assert_eq!(validate(value).unwrap_err().pointer, "/reply/sinks/0/url");
    }
}

#[test]
fn private_channel_roundtrip_and_public_view_never_confuse_credential_custody() {
    let mut value = channel();
    for key in ["channel_key", "provider_credential"] {
        value["secrets"][key] = json!(format!("fixture-{key}"));
    }
    value["secrets"]["hmac_secrets"] = json!([["key", "fixture-hmac-value"]]);
    let channel: ChannelDefinition = serde_json::from_value(value).unwrap();
    channel.validate().unwrap();
    let blob = channel.to_blob_json();
    let decoded: ChannelDefinition = serde_json::from_value(blob.clone()).unwrap();
    assert_eq!(decoded, channel);
    assert_eq!(decoded.to_blob_json(), blob);
    let public = channel.to_public_json();
    assert_eq!(
        public["secrets"],
        json!({"agent_key_hash":channel.secrets.agent_key_hash})
    );
    for marker in [
        "fixture-channel_key",
        "fixture-provider_credential",
        "fixture-hmac-value",
    ] {
        assert!(!public.to_string().contains(marker));
        assert!(!format!("{channel:?}").contains(marker));
        assert!(!serde_json::to_string(&channel).unwrap().contains(marker));
        assert!(blob.to_string().contains(marker));
    }
}

fn relay_on(platform: &str, fields: Value) -> Value {
    let mut value = relay_channel();
    value["sources"][0]["platform"] = json!(platform);
    for (key, field) in fields.as_object().unwrap() {
        value["sources"][0][key] = field.clone();
    }
    value
}

#[test]
fn relay_platforms_parse_their_nyxid_spellings_and_serialize_canonically() {
    use cmaeg_core::channel::Platform;
    for (wire, platform) in [
        ("telegram", Platform::Telegram),
        ("telegram-new", Platform::Telegram),
        ("lark", Platform::Lark),
        ("feishu", Platform::Feishu),
        ("discord", Platform::Discord),
        ("slack", Platform::Slack),
        ("whatsapp", Platform::Whatsapp),
    ] {
        assert_eq!(wire.parse::<Platform>().unwrap(), platform);
        assert!(platform.matches_wire(wire));
        assert_eq!(serde_json::to_value(platform).unwrap(), platform.as_str());
        assert_eq!(
            serde_json::from_value::<Platform>(json!(platform.as_str())).unwrap(),
            platform
        );
    }
    for wire in ["", "Lark", "telegram_new", "whats_app", "wechat"] {
        assert!(wire.parse::<Platform>().is_err(), "{wire}");
    }
    assert!(!Platform::Lark.matches_wire("feishu"));
    assert!(!Platform::Feishu.matches_wire("lark"));
    // `telegram-new` is a NyxID transport spelling, not a protocol value.
    assert!(serde_json::from_value::<Platform>(json!("telegram-new")).is_err());
}

#[test]
fn bot_identity_is_validated_per_platform() {
    for (platform, field, valid, invalid) in [
        (
            "telegram",
            "bot_id",
            vec!["1", "12345678901234567890"],
            vec!["", "ou_bot", "-1", "123456789012345678901"],
        ),
        (
            "lark",
            "bot_id",
            vec!["ou_7d8a6e6df7621556ce0d21922b676706"],
            vec!["", "ou_", "cli_a1b2", "on_union", "ou_bad id", "OU_upper"],
        ),
        (
            "feishu",
            "bot_id",
            vec!["ou_bot-1.2"],
            vec!["ou_bot@tenant"],
        ),
        (
            "slack",
            "bot_id",
            vec!["B012ABCDEF", "U0.1-x_y"],
            vec!["", "bad id", "B:1"],
        ),
        (
            "telegram",
            "bot_username",
            vec!["support_bot"],
            vec!["bot", "support-bot", "@support_bot"],
        ),
        (
            "lark",
            "bot_username",
            vec!["s", "support-bot.v2"],
            vec!["", "@support", "Support Bot", "客服"],
        ),
        (
            "whatsapp",
            "bot_username",
            vec!["support_bot"],
            vec!["support bot"],
        ),
    ] {
        for value in valid {
            validate(relay_on(platform, json!({ field: value })))
                .unwrap_or_else(|error| panic!("{platform} {field} {value}: {error}"));
        }
        for value in invalid {
            assert_eq!(
                validate(relay_on(platform, json!({ field: value })))
                    .unwrap_err()
                    .pointer,
                format!("/sources/0/{field}"),
                "{platform} {field} {value}"
            );
        }
    }
    for (field, max) in [("bot_id", 128), ("bot_username", 64)] {
        validate(relay_on("discord", json!({ field: "x".repeat(max) }))).unwrap();
        assert_eq!(
            validate(relay_on("discord", json!({ field: "x".repeat(max + 1) })))
                .unwrap_err()
                .pointer,
            format!("/sources/0/{field}")
        );
    }
    validate(relay_on(
        "lark",
        json!({"bot_id": format!("ou_{}", "x".repeat(125))}),
    ))
    .unwrap();
    assert_eq!(
        validate(relay_on(
            "lark",
            json!({"bot_id": format!("ou_{}", "x".repeat(126))})
        ))
        .unwrap_err()
        .pointer,
        "/sources/0/bot_id"
    );
}

#[test]
fn trust_normalized_is_only_for_platforms_without_a_raw_event_verifier() {
    for platform in ["discord", "slack", "whatsapp"] {
        validate(relay_on(platform, json!({"trust_normalized": true}))).unwrap();
        // Stored definitions without the opt-in stay restorable so ingress can refuse
        // them explicitly; channel management refuses to create or update one.
        validate(relay_on(platform, json!({"trust_normalized": false}))).unwrap();
    }
    for platform in ["telegram", "lark", "feishu"] {
        validate(relay_on(platform, json!({"trust_normalized": false}))).unwrap();
        assert_eq!(
            validate(relay_on(platform, json!({"trust_normalized": true})))
                .unwrap_err()
                .pointer,
            "/sources/0/trust_normalized"
        );
    }
    let value = relay_on("slack", json!({"trust_normalized": "yes"}));
    assert!(serde_json::from_value::<ChannelDefinition>(value).is_err());
}

#[test]
fn relay_identity_fields_are_omitted_when_unset_and_retained_when_set() {
    let legacy: ChannelDefinition = serde_json::from_value(relay_channel()).unwrap();
    let source = &legacy.to_blob_json()["sources"][0];
    assert!(source.get("bot_id").is_none());
    assert!(source.get("trust_normalized").is_none());
    assert_eq!(
        legacy.relay_platform(),
        Some(cmaeg_core::channel::Platform::Telegram)
    );
    let value = relay_on(
        "slack",
        json!({"bot_id":"B012ABCDEF","bot_username":"support","trust_normalized":true}),
    );
    let definition: ChannelDefinition = serde_json::from_value(value).unwrap();
    definition.validate().unwrap();
    let blob = definition.to_blob_json();
    assert_eq!(blob["sources"][0]["bot_id"], "B012ABCDEF");
    assert_eq!(blob["sources"][0]["trust_normalized"], true);
    assert_eq!(
        definition.to_public_json()["sources"][0]["platform"],
        "slack"
    );
    assert_eq!(
        serde_json::from_value::<ChannelDefinition>(blob).unwrap(),
        definition
    );
    assert_eq!(
        serde_json::from_value::<ChannelDefinition>(channel())
            .unwrap()
            .relay_platform(),
        None
    );
}

#[test]
fn legacy_rules_allow_at_most_one_relay_source() {
    let mut value = relay_channel();
    let mut lark = relay();
    lark["platform"] = json!("lark");
    value["sources"] = json!([relay(), lark]);
    let error = validate(value).unwrap_err();
    assert_eq!(
        (error.pointer.as_str(), error.message.as_str()),
        ("/sources/1", "source variant is duplicated")
    );
}

#[test]
fn native_bot_mentions_require_raw_native_verification_and_pinned_receiver() {
    for platform in ["telegram", "lark", "feishu", "slack"] {
        for (identity, trusted, allowed) in [
            (None, false, false),
            (Some("display-name"), false, false),
            (
                Some(if platform == "telegram" {
                    "77"
                } else {
                    "ou_receiver"
                }),
                false,
                platform != "slack",
            ),
            (
                Some(if platform == "telegram" {
                    "77"
                } else {
                    "ou_receiver"
                }),
                true,
                false,
            ),
        ] {
            let mut value = relay_channel();
            let source = &mut value["sources"][0];
            source["platform"] = json!(platform);
            source["bot_id"] = json!(identity);
            if platform == "telegram" {
                source["bot_username"] = json!("receiver_bot");
            }
            source["trust_normalized"] = json!(trusted);
            source["admission"] = json!({"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":"mention","bot_messages":"native_mentions"});
            let definition: ChannelDefinition = serde_json::from_value(value).unwrap();
            assert_eq!(
                definition.validate().is_ok(),
                allowed,
                "{platform} {identity:?} trusted={trusted}"
            );
        }
    }
    let definition: ChannelDefinition = serde_json::from_value(relay_channel()).unwrap();
    assert!(!definition.sources[0].native_bot_mentions());
    assert!(
        serde_json::to_value(&definition).unwrap()["sources"][0]["admission"]
            .get("bot_messages")
            .is_none()
    );
}
