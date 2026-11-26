//! The write gate through the public management API: no management path can
//! store a shape an older image cannot read, and every refusal keeps the
//! exact problem body callers received before readers widened.
use super::*;
use cmaeg::channels::definition::{channel_pointer, hash_secret};
use cmaeg_core::{
    channel::ChannelDefinition,
    ids::RecordId,
    profile::{canonical_json, EventWorkerProfile},
    seal::{open_blob, seal_blob},
};
use serde_json::Value;

const DUPLICATED: &str = "/sources/1: source variant is duplicated";
const CUSTOM_KEY_DISABLED: &str = "/partition/custom_key: dynamic_sources_disabled";

pub(super) fn relay_request(platform: &str, route: &str) -> Value {
    let mut source = json!({"type":"nyxid_relay","issuer":"https://nyx.example","key_id":"route-key",
        "route_ids":[route],"platform":platform,"admission":{"type":"all"}});
    if matches!(platform, "discord" | "slack" | "whatsapp") {
        source["trust_normalized"] = true.into();
    }
    source
}

fn lark_relay() -> Value {
    relay_request("lark", "route-2")
}

/// A create body carrying a second relay next to the Telegram one.
fn two_relays() -> Value {
    let mut body = create_body();
    body["sources"].as_array_mut().unwrap().push(lark_relay());
    body
}

async fn refused(response: reqwest::Response, detail: &str) {
    assert_eq!(response.status(), 400);
    let problem: Value = response.json().await.unwrap();
    assert_eq!(problem["code"], "invalid_request", "{problem}");
    assert_eq!(problem["detail"], detail, "{problem}");
}

pub(super) struct Gateway {
    provider: FakeProvider,
    pub(super) cma: FakeCma,
    nyxid: FakeNyxid,
    pub(super) base: String,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
    pub(super) client: Client,
}

impl Gateway {
    pub(super) async fn start() -> Self {
        Self::start_with_bots(false).await
    }

    async fn start_with_bots(enabled: bool) -> Self {
        let provider = FakeProvider::start(FakeProviderConfig::default()).await;
        let cma = FakeCma::start(false).await;
        let nyxid = FakeNyxid::start().await;
        let mut state = management_state(&provider, &cma, &nyxid, key_ring(1));
        state.native_bot_mentions_writes_enabled = enabled;
        let (base, stop, task) = serve_app(state).await;
        Self {
            provider,
            cma,
            nyxid,
            base,
            stop,
            task,
            client: Client::new(),
        }
    }

    async fn create(&self, body: &Value) -> reqwest::Response {
        self.client
            .post(format!("{}/v1/channels", self.base))
            .bearer_auth("token-alice")
            .json(body)
            .send()
            .await
            .unwrap()
    }

    pub(super) async fn update(&self, channel_id: &str, body: &Value) -> reqwest::Response {
        self.client
            .put(format!("{}/v1/channels/{channel_id}", self.base))
            .bearer_auth("token-alice")
            .json(body)
            .send()
            .await
            .unwrap()
    }

    pub(super) async fn close(self) {
        self.stop.send(()).unwrap();
        self.task.await.unwrap();
        self.provider.close().await;
        self.cma.close().await;
        self.nyxid.close().await;
    }
}

#[tokio::test]
async fn creating_a_second_relay_keeps_the_legacy_refusal() {
    let gateway = Gateway::start().await;
    for reserved in [false, true] {
        let mut body = two_relays();
        if reserved {
            body["record_id"] = uuid::Uuid::new_v4().to_string().into();
        }
        refused(gateway.create(&body).await, DUPLICATED).await;
    }
    assert!(gateway.provider.requests().is_empty());
    assert!(gateway.cma.records().is_empty());
    gateway.close().await;
}

#[tokio::test]
async fn updating_to_a_second_relay_keeps_the_legacy_refusal() {
    let gateway = Gateway::start().await;
    let created = created(&gateway.client, &gateway.base).await;
    let before = gateway.cma.records();
    let calls = gateway.provider.requests().len();
    let mut update = two_relays();
    update.as_object_mut().unwrap().remove("credentials");
    update["expected_version"] = created["version"].clone();
    let channel_id = created["channel_id"].as_str().unwrap();
    refused(gateway.update(channel_id, &update).await, DUPLICATED).await;
    assert_eq!(gateway.provider.requests().len(), calls);
    let after = gateway.cma.records();
    for (record_id, record) in &before {
        assert_eq!(after[record_id].version, record.version);
        assert_eq!(after[record_id].blob, record.blob);
    }
    gateway.close().await;
}

#[tokio::test]
async fn custom_key_writes_are_refused_while_dynamic_sources_are_off() {
    let gateway = Gateway::start().await;
    let mut body = create_body();
    body["partition"]["custom_key"] = "event_subject".into();
    for reserved in [false, true] {
        let mut body = body.clone();
        if reserved {
            body["record_id"] = uuid::Uuid::new_v4().to_string().into();
        }
        refused(gateway.create(&body).await, CUSTOM_KEY_DISABLED).await;
    }
    assert!(gateway.provider.requests().is_empty());
    assert!(gateway.cma.records().is_empty());
    // An update introducing it to a legacy channel is refused the same way,
    // while a second relay still gets the legacy error.
    let created = created(&gateway.client, &gateway.base).await;
    let channel_id = created["channel_id"].as_str().unwrap();
    body.as_object_mut().unwrap().remove("credentials");
    body["expected_version"] = created["version"].clone();
    refused(gateway.update(channel_id, &body).await, CUSTOM_KEY_DISABLED).await;
    refused(gateway.create(&two_relays()).await, DUPLICATED).await;
    gateway.close().await;
}

/// Store a Lark plus Feishu channel for alice directly, as a writer behind
/// the dynamic-sources flag would have, and return its channel and record ids.
fn seed_two_relays(gateway: &Gateway) -> (String, String) {
    seed(gateway, |_| {})
}

/// [`seed_two_relays`] after `edit`.
fn seed(gateway: &Gateway, edit: impl FnOnce(&mut Value)) -> (String, String) {
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
    value["provider"]["binding_id"] = format!("bnd_{}", uuid::Uuid::new_v4().simple()).into();
    value["sources"] = json!([
        relay_request("lark", "route-1"),
        relay_request("feishu", "route-2")
    ]);
    for source in value["sources"].as_array_mut().unwrap() {
        source["jwks_url"] = "https://nyx.example/.well-known/jwks.json".into();
    }
    value["partition"]["key"] = "conversation_and_sender".into();
    value["secrets"] = json!({"agent_key_hash": hash_secret("nyx_agent_secret"),
        "provider_credential": "nyx_agent_secret", "channel_key": "nyx_route_secret"});
    edit(&mut value);
    let definition: ChannelDefinition = serde_json::from_value(value).unwrap();
    definition.validate().unwrap();
    assert!(definition.validate_legacy().is_err());
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

/// A full PUT that carries forward the pinned profile and the seeded sources.
fn full_put(profile: &Value, edit: impl FnOnce(&mut Value)) -> Value {
    let mut body = create_body();
    body.as_object_mut().unwrap().remove("credentials");
    body["profile"] = profile.clone();
    body["sources"] = json!([
        relay_request("lark", "route-1"),
        relay_request("feishu", "route-2")
    ]);
    body["expected_version"] = 1.into();
    edit(&mut body);
    body
}

#[tokio::test]
async fn an_old_clients_full_put_carries_an_extended_channel_through() {
    let gateway = Gateway::start().await;
    let (channel_id, record_id) = seed_two_relays(&gateway);
    let read = gateway
        .client
        .get(format!("{}/v1/channels/{channel_id}", gateway.base))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(read.status(), 200);
    let read: Value = read.json().await.unwrap();
    assert_eq!(read["definition"]["sources"].as_array().unwrap().len(), 2);
    let profile = &read["definition"]["profile"];

    // Adding a third relay modifies the extension and gets the legacy error.
    let third = full_put(profile, |body| {
        body["sources"]
            .as_array_mut()
            .unwrap()
            .push(relay_request("discord", "route-3"));
    });
    refused(gateway.update(&channel_id, &third).await, DUPLICATED).await;
    assert_eq!(gateway.cma.records()[&record_id].version, 1);

    // Changing only the name carries the sources through with the flag off.
    let renamed = full_put(profile, |body| body["name"] = "renamed-bot".into());
    let response = gateway.update(&channel_id, &renamed).await;
    assert_eq!(response.status(), 200);
    let stored = gateway.cma.records()[&record_id].clone();
    assert_eq!(stored.version, 2);
    let opened = open_blob(&key_ring(1), &stored.blob).unwrap().payload;
    let definition: ChannelDefinition = serde_json::from_slice(&opened).unwrap();
    assert_eq!(definition.name, "renamed-bot");
    assert_eq!(definition.sources.len(), 2);
    definition.validate().unwrap();
    gateway.close().await;
}

#[tokio::test]
async fn a_channel_without_sources_is_still_readable() {
    let gateway = Gateway::start().await;
    let (channel_id, _) = seed(&gateway, |value| {
        value["sources"] = json!([]);
        value["reply"]["sinks"] = json!([{"type":"sync"}]);
    });
    let response = gateway
        .client
        .get(format!("{}/v1/channels/{channel_id}", gateway.base))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let read: Value = response.json().await.unwrap();
    assert_eq!(read["definition"]["sources"], json!([]));
    gateway.close().await;
}

#[tokio::test]
async fn external_native_bot_mentions_writes_are_explicit_opt_in_and_removable() {
    for enabled in [false, true] {
        let gateway = Gateway::start_with_bots(enabled).await;
        let mut body = create_body();
        body["sources"][0]["bot_id"] = json!("77");
        body["sources"][0]["bot_username"] = json!("receiver_bot");
        body["sources"][0]["admission"] = json!({"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":"mention","bot_messages":"native_mentions"});
        let response = gateway.create(&body).await;
        if !enabled {
            refused(response, "/sources: native_bot_mentions_writes_disabled").await;
            assert!(gateway.cma.records().is_empty());
            assert!(gateway.provider.requests().is_empty());
        } else {
            assert_eq!(response.status(), 201);
            let created: Value = response.json().await.unwrap();
            let read: Value = gateway
                .client
                .get(format!(
                    "{}/v1/channels/{}",
                    gateway.base,
                    created["channel_id"].as_str().unwrap()
                ))
                .bearer_auth("token-alice")
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(
                read["definition"]["sources"][0]["admission"]["bot_messages"],
                "native_mentions"
            );
            // A reader with new writes disabled can carry the source through
            // unchanged and then remove the opt-in before an old-reader rollback.
            let (base, stop, task) = serve_app(management_state(
                &gateway.provider,
                &gateway.cma,
                &gateway.nyxid,
                key_ring(1),
            ))
            .await;
            body.as_object_mut().unwrap().remove("credentials");
            body["expected_version"] = created["version"].clone();
            let url = format!(
                "{base}/v1/channels/{}",
                created["channel_id"].as_str().unwrap()
            );
            let carried = gateway
                .client
                .put(&url)
                .bearer_auth("token-alice")
                .json(&body)
                .send()
                .await
                .unwrap();
            assert_eq!(carried.status(), 200);
            let carried: Value = carried.json().await.unwrap();
            body["expected_version"] = carried["version"].clone();
            body["sources"][0]["admission"]
                .as_object_mut()
                .unwrap()
                .remove("bot_messages");
            let removed = gateway
                .client
                .put(&url)
                .bearer_auth("token-alice")
                .json(&body)
                .send()
                .await
                .unwrap();
            assert_eq!(removed.status(), 200);
            let removed: Value = removed.json().await.unwrap();
            assert!(removed["definition"]["sources"][0]["admission"]
                .get("bot_messages")
                .is_none());
            stop.send(()).unwrap();
            task.await.unwrap();
        }
        gateway.close().await;
    }
}
