use cmaeg_core::{channel::ChannelDefinition, profile::EventWorkerProfile};
use serde_json::{json, Value};

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

fn validate(value: Value) -> Result<(), cmaeg_core::ValidationError> {
    serde_json::from_value::<ChannelDefinition>(value)
        .unwrap()
        .validate()
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
