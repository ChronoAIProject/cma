use axum::http::HeaderMap;
use cmaeg::{
    auth::webhook,
    ingress::custom::{authenticated, template},
};
use cmaeg_core::{
    channel::{ChannelDefinition, ChannelSecrets, CustomAuth, PartitionKey, Secret},
    cloudevent::CloudEvent,
};
use serde_json::json;
use sha2::{Digest, Sha256};

#[test]
fn standard_webhooks_authenticates_exact_body_and_refuses_bad_stale_or_ambiguous_headers() {
    let secret = Secret::new(uuid::Uuid::new_v4().to_string());
    let body = b"{\"a\":1}";
    let timestamp = "2000000000";
    let mut headers = HeaderMap::new();
    headers.insert("webhook-id", "event-1".parse().unwrap());
    headers.insert("webhook-timestamp", timestamp.parse().unwrap());
    let signature = webhook::signature(&secret, "event-1", timestamp, body);
    headers.insert(
        "webhook-signature",
        format!("v2,ignored v1,invalid {signature}")
            .parse()
            .unwrap(),
    );
    let auth = CustomAuth::Hmac {
        key_id: "key-1".into(),
    };
    let secrets = ChannelSecrets {
        hmac_secrets: vec![("key-1".into(), secret.clone())],
        ..Default::default()
    };
    assert!(authenticated(&auth, &secrets, &headers, body, 2000000000));
    assert!(!authenticated(
        &auth,
        &secrets,
        &headers,
        b"{\"a\":2}",
        2000000000
    ));
    assert!(!webhook::verify(
        &headers,
        &Secret::new("different"),
        body,
        2000000000
    ));
    assert!(!authenticated(&auth, &secrets, &headers, body, 2000000301));
    assert!(!authenticated(&auth, &secrets, &headers, body, 1999999699));
    for name in ["webhook-id", "webhook-timestamp", "webhook-signature"] {
        let mut missing = headers.clone();
        missing.remove(name);
        assert!(!authenticated(&auth, &secrets, &missing, body, 2000000000));
        let mut duplicate = headers.clone();
        duplicate.append(name, "duplicate".parse().unwrap());
        assert!(!authenticated(
            &auth, &secrets, &duplicate, body, 2000000000
        ));
    }
}
#[test]
fn nyxid_source_keys_are_local_hash_matches_and_require_one_bearer() {
    let key = format!("nyx_{}", uuid::Uuid::new_v4().simple());
    let auth = CustomAuth::NyxidKey {
        key_hashes: vec![format!("sha256:{:x}", Sha256::digest(key.as_bytes()))],
    };
    let mut headers = HeaderMap::new();
    let secrets = ChannelSecrets::default();
    assert!(!authenticated(&auth, &secrets, &headers, b"", 0));
    headers.insert("authorization", format!("Bearer {key}").parse().unwrap());
    assert!(authenticated(&auth, &secrets, &headers, b"", 0));
    headers.append("authorization", format!("Bearer {key}").parse().unwrap());
    assert!(!authenticated(&auth, &secrets, &headers, b"", 0));
    headers.insert("authorization", "Bearer rejected".parse().unwrap());
    assert!(!authenticated(&auth, &secrets, &headers, b"", 0));
}
#[test]
fn templates_substitute_once_preserve_unknowns_and_bound_unicode_data_and_total_output() {
    let mut event: CloudEvent = serde_json::from_value(json!({"specversion":"1.0","id":"id-1","source":"{{id}}","type":"order.created","subject":"subject","time":"2026-09-22T00:00:00Z","data":{"z":1,"a":2}})).unwrap();
    let output = template::render(
        "{{type}} {{source}} {{subject}} {{id}} {{time}} {{data}} {{unknown}}",
        &event,
    );
    assert_eq!(
        output,
        "order.created {{id}} subject id-1 2026-09-22T00:00:00Z {\"a\":2,\"z\":1} {{unknown}}"
    );
    event.data = Some(json!({"text":"🦀".repeat(8192)}));
    let output = template::render("{{data}}", &event);
    assert!(output.len() <= 8192 && output.ends_with("[truncated]"));
    let output = template::render(&"{{data}}".repeat(1024), &event);
    assert!(output.len() <= 32768 && output.ends_with("[truncated]"));
    assert_eq!(template::render("{{broken", &event), "{{broken");
}
#[test]
fn custom_channels_reject_chat_partition_rules_at_creation() {
    let mut channel: ChannelDefinition =
        serde_json::from_str(include_str!("../../cmaeg-core/tests/fixtures/channel.json")).unwrap();
    channel.profile_digest = channel.profile.digest();
    for key in [
        PartitionKey::Conversation,
        PartitionKey::ConversationAndSender,
        PartitionKey::Thread,
    ] {
        channel.partition.key = key;
        assert!(channel.validate().is_err());
    }
    for key in [
        PartitionKey::Fixed,
        PartitionKey::EventSubject,
        PartitionKey::PerEvent,
    ] {
        channel.partition.key = key;
        channel.validate().unwrap();
    }
}
