//! A routed channel through the public management API. No request can store
//! routing yet, so the channel is stored as a writer behind the
//! multi-consumer flag would store it: three relays and a custom source,
//! served by `default` and two additional consumers.
use super::write_gate::{relay_request, Gateway};
use super::*;
use cmaeg::channels::definition::{channel_pointer, hash_secret};
use cmaeg_core::{
    channel::ChannelDefinition,
    ids::RecordId,
    profile::{canonical_json, EventWorkerProfile},
    seal::{open_blob, seal_blob},
};
use serde_json::Value;
use std::sync::Mutex;

const KEYS: [(&str, &str); 2] = [
    ("csm_00000000000a", "nyx_reviewer_key"),
    ("csm_00000000000b", "nyx_nyxbot_key"),
];

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn sources() -> Value {
    json!([
        relay_request("lark", "route-1"),
        relay_request("feishu", "route-2"),
        relay_request("telegram", "route-3"),
        {"type": "custom", "auth": {"type": "hmac", "key_id": "h1"}}
    ])
}

fn consumer(id: &str, name: &str, provider: &str, key: &str, profile: &Value) -> Value {
    let mut profile = profile.clone();
    profile["metadata"]["provider"] = provider.into();
    let digest = serde_json::from_value::<EventWorkerProfile>(profile.clone())
        .unwrap()
        .digest();
    json!({"id": id, "name": name, "label": name.to_uppercase(), "profile": profile,
        "profile_digest": digest,
        "provider": {"slug": provider, "binding_id": format!("bnd_{name}"), "level": "l1"},
        "agent_key_hash": hash_secret(key), "created_at": "2026-10-01T00:00:00Z"})
}

/// Store the routed channel for alice and return its channel and record ids.
fn seed(gateway: &Gateway) -> (String, String) {
    let record_id = uuid::Uuid::new_v4().to_string();
    let mut value: Value = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    value["record_id"] = record_id.clone().into();
    value["owner"]["subject"] = "alice".into();
    value["profile"]["metadata"]["owner"]["subject"] = "alice".into();
    let profile: EventWorkerProfile = serde_json::from_value(value["profile"].clone()).unwrap();
    value["profile_digest"] = profile.digest().into();
    value["sources"] = sources();
    for source in value["sources"].as_array_mut().unwrap() {
        if source["type"] == "nyxid_relay" {
            source["jwks_url"] = "https://nyx.example/.well-known/jwks.json".into();
        }
    }
    value["partition"] =
        json!({"version": 1, "key": "conversation", "custom_key": "event_subject"});
    let hmac = base64::engine::general_purpose::STANDARD.encode(b"hmac-secret");
    value["secrets"] = json!({"agent_key_hash": hash_secret("nyx_agent_secret"),
        "provider_credential": "nyx_agent_secret", "channel_key": "nyx_route_secret",
        "hmac_secrets": [["h1", hmac]],
        "consumer_keys": KEYS.iter().map(|(id, key)| json!([id, key])).collect::<Vec<_>>()});
    value["routing"] = json!({"version": 1, "default_consumer": {"name": "lead"},
        "consumers": [
            consumer(KEYS[0].0, "reviewer", "cma_codex", KEYS[0].1, &value["profile"]),
            consumer(KEYS[1].0, "nyx", "nyxbot", KEYS[1].1, &value["profile"])
        ],
        "rules": [{"id": "lark", "when": {"sources": ["relay_lark"]}, "targets": ["default", KEYS[0].0]}],
        "default_targets": ["default"],
        "default_reply": {"type": "sinks"}});
    value["reply"]["sinks"] = json!([{"type": "source"},
        {"type": "result_webhook", "url": "https://receiver.example/events", "hmac_key_id": "h1"}]);
    let definition: ChannelDefinition = serde_json::from_value(value).unwrap();
    definition.validate().unwrap();
    let keys = key_ring(1);
    let blob = seal_blob(&keys, canonical_json(definition.to_blob_json()).as_bytes()).unwrap();
    gateway.cma.seed(&record_id, "alice", blob);
    let channel_id = channel_pointer(
        &gateway.cma.base_url,
        &RecordId::new(record_id.clone()).unwrap(),
    )
    .unwrap()
    .seal_id(&keys)
    .unwrap();
    (channel_id.as_str().to_owned(), record_id)
}

fn stored(gateway: &Gateway, record_id: &str) -> ChannelDefinition {
    let record = gateway.cma.records()[record_id].clone();
    let opened = open_blob(&key_ring(1), &record.blob).unwrap().payload;
    serde_json::from_slice(&opened).unwrap()
}

/// The routed channel reads back with its policy and only public key hashes,
/// and an older client's full PUT carries the policy and its keys through.
/// The fake backends run on this current-thread runtime, so the
/// thread-scoped subscriber captures every trace line of both requests.
#[tokio::test(flavor = "current_thread")]
async fn a_routed_channel_reads_back_with_only_hashes_and_survives_a_full_put() {
    keep_log_callsite_registry_shared();
    let capture = Capture::default();
    let writer = capture.clone();
    let _logs = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    );
    let gateway = Gateway::start().await;
    let (channel_id, record_id) = seed(&gateway);
    let before = stored(&gateway, &record_id);

    let response = gateway
        .client
        .get(format!("{}/v1/channels/{channel_id}", gateway.base))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = response.text().await.unwrap();
    let read: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        read["definition"]["routing"],
        serde_json::to_value(&before.routing).unwrap()
    );
    for (id, key) in KEYS {
        assert!(body.contains(&hash_secret(key)), "{id}");
    }
    assert!(!body.contains("consumer_keys"));

    let mut put = create_body();
    put.as_object_mut().unwrap().remove("credentials");
    put["profile"] = read["definition"]["profile"].clone();
    put["sources"] = sources();
    put["partition"] = serde_json::to_value(&before.partition).unwrap();
    put["reply"] = serde_json::to_value(&before.reply).unwrap();
    put["name"] = "renamed-bot".into();
    put["expected_version"] = 1.into();
    let response = gateway.update(&channel_id, &put).await;
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(status, 200, "{body}");
    let after = stored(&gateway, &record_id);
    assert_eq!(after.name, "renamed-bot");
    assert_eq!(after.routing, before.routing);
    assert_eq!(after.secrets.consumer_keys, before.secrets.consumer_keys);
    after.validate().unwrap();

    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("TRACE") || logs.contains("DEBUG"),
        "trace logs were captured"
    );
    for (_, key) in KEYS {
        assert!(!body.contains(key));
        assert!(!logs.contains(key), "a consumer key reached the logs");
    }
    gateway.close().await;
}
