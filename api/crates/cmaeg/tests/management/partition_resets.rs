use super::binary_support::{assert_no_secrets, free_address, spawn_ready, MountedSecrets};
use super::*;
use cmaeg_core::{
    channel::ChannelDefinition,
    ids::{ChannelId, ConsumerId, ConversationId},
    profile::canonical_json,
    seal::{open_blob, seal_blob},
};
use mongodb::bson::{doc, Document};
use serde_json::Value;
use std::sync::atomic::Ordering;

pub(super) fn body() -> Value {
    json!({"partition_key":"repair-fixture","expected_version":1,"expected_agent_id":"agt_deleted"})
}
pub(super) fn request(client: &Client, base: &str, id: &str) -> reqwest::RequestBuilder {
    client
        .post(format!("{base}/v1/channels/{id}/partition/resets"))
        .bearer_auth("token-alice")
}
pub(super) async fn invalid_response(request: reqwest::RequestBuilder) {
    response(request.send().await.unwrap(), 400).await;
}
pub(super) async fn response(response: reqwest::Response, status: u16) -> Value {
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()["cache-control"], "no-store");
    if status != 200 {
        assert_eq!(
            response.headers()["content-type"],
            "application/problem+json"
        );
    }
    let text = response.text().await.unwrap();
    assert_no_secrets(&text);
    serde_json::from_str(&text).unwrap()
}

struct Gateway {
    provider: FakeProvider,
    cma: FakeCma,
    nyxid: FakeNyxid,
    files: MountedSecrets,
    database: database::DatabaseFixture,
    child: tokio::process::Child,
    base: String,
    client: Client,
}
impl Gateway {
    async fn new(config: FakeProviderConfig) -> Self {
        let provider = FakeProvider::start(config).await;
        let cma = FakeCma::start(false).await;
        let nyxid = FakeNyxid::start().await;
        let files = MountedSecrets::new();
        let database =
            database::DatabaseFixture::new(std::path::Path::new(env!("CARGO_BIN_EXE_cmaeg"))).await;
        let address = free_address();
        let mut command = files.command(&address, &cma, &nyxid, &provider);
        database.configure(&mut command);
        command.env("CMAEG_CMA_BASE_URL", provider.base_url());
        let child = spawn_ready(command, &address, &files.dir.join("trace.log")).await;
        Self {
            provider,
            cma,
            nyxid,
            files,
            database,
            child,
            base: format!("http://{address}"),
            client: Client::new(),
        }
    }
    async fn restart(&mut self) {
        self.child.kill().await.unwrap();
        self.child.wait().await.unwrap();
        let address = free_address();
        let mut command = self
            .files
            .command(&address, &self.cma, &self.nyxid, &self.provider);
        self.database.configure(&mut command);
        command.env("CMAEG_CMA_BASE_URL", self.provider.base_url());
        self.child =
            spawn_ready(command, &address, &self.files.dir.join("trace-restart.log")).await;
        self.base = format!("http://{address}");
    }
    fn records(&self) -> mongodb::Collection<Document> {
        self.database
            .admin
            .database(&self.database.name)
            .collection("eg_channel_records")
    }
    async fn record(&self, channel: &Value) -> Document {
        self.records()
            .find_one(doc! {"_id":channel["record_id"].as_str().unwrap()})
            .await
            .unwrap()
            .unwrap()
    }
    async fn replace(&self, original: &Document, mutate: impl FnOnce(&mut Value)) {
        let mut next = original.clone();
        let bytes = original.get_binary_generic("blob").unwrap();
        let opened = open_blob(&key_ring(1), bytes).unwrap().payload;
        let mut definition: Value = serde_json::from_slice(&opened).unwrap();
        mutate(&mut definition);
        let definition: ChannelDefinition = serde_json::from_value(definition).unwrap();
        definition.validate().unwrap();
        let blob = seal_blob(
            &key_ring(1),
            canonical_json(definition.to_blob_json()).as_bytes(),
        )
        .unwrap();
        next.insert(
            "blob",
            mongodb::bson::Binary {
                subtype: mongodb::bson::spec::BinarySubtype::Generic,
                bytes: blob,
            },
        );
        next.insert("owner", definition.owner.subject);
        self.records()
            .replace_one(doc! {"_id":original.get_str("_id").unwrap()}, next)
            .await
            .unwrap();
    }
    async fn reset(&self, id: &str, key: &str, body: &Value, status: u16) -> Value {
        response(
            request(&self.client, &self.base, id)
                .header("Idempotency-Key", key)
                .json(body)
                .send()
                .await
                .unwrap(),
            status,
        )
        .await
    }
    async fn inspect(&self, id: &str, partition: &str, consumer: &str) -> Value {
        response(
            self.client
                .get(format!("{}/v1/channels/{id}/partition", self.base))
                .bearer_auth("token-alice")
                .query(&[("partition_key", partition), ("consumer_id", consumer)])
                .send()
                .await
                .unwrap(),
            200,
        )
        .await
    }
    async fn close(mut self) {
        self.child.kill().await.unwrap();
        self.child.wait().await.unwrap();
        for name in ["trace.log", "trace-restart.log"] {
            if let Ok(log) = std::fs::read_to_string(self.files.dir.join(name)) {
                assert_no_secrets(&log);
            }
        }
        self.database
            .admin
            .database(&self.database.name)
            .drop()
            .await
            .unwrap();
        self.provider.close().await;
        self.cma.close().await;
        self.nyxid.close().await;
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn compiled_reset_replays_after_loss_restart_gate_closure_and_preserves_channel() {
    let mut gateway = Gateway::new(FakeProviderConfig {
        inspection_agent: Some(("agt_deleted".into(), "deleted".into())),
        ..Default::default()
    })
    .await;
    let channel = created(&gateway.client, &gateway.base).await;
    let id = channel["channel_id"].as_str().unwrap();
    let before = gateway.record(&channel).await;
    let healthy = gateway.inspect(id, "healthy", "default").await;
    assert_eq!(
        gateway.reset(id, "repair", &body(), 409).await["detail"],
        "partition_reset_disabled"
    );
    assert_eq!(gateway.provider.reset_count(), 0);
    let controls = gateway.provider.reset_controls();
    controls.enabled.store(true, Ordering::SeqCst);
    controls.lose_once.store(true, Ordering::SeqCst);
    gateway.reset(id, "repair", &body(), 503).await;
    assert_eq!(gateway.provider.reset_count(), 1);
    gateway.restart().await;
    let receipt = gateway.reset(id, "repair", &body(), 200).await;
    assert_eq!(receipt["reset"]["state"], "reserved");
    controls.enabled.store(false, Ordering::SeqCst);
    assert_eq!(gateway.reset(id, "repair", &body(), 200).await, receipt);
    assert_eq!(
        gateway.reset(id, "new-key", &body(), 409).await["detail"],
        "partition_reset_disabled"
    );
    let inspected = gateway.inspect(id, "repair-fixture", "default").await;
    assert_eq!(
        inspected["mapping"]["agent_id"],
        receipt["reset"]["successor_agent_id"]
    );
    assert_eq!(inspected["mapping"]["incarnation"], 1);
    assert_eq!(gateway.inspect(id, "healthy", "default").await, healthy);
    assert_eq!(gateway.record(&channel).await, before);
    assert_eq!(gateway.records().count_documents(doc! {}).await.unwrap(), 1);
    *controls.response.lock().unwrap() = Some((200, json!({"state":"ready"})));
    gateway.reset(id, "repair", &body(), 503).await;
    *controls.response.lock().unwrap() = None;
    assert_eq!(gateway.reset(id, "repair", &body(), 200).await, receipt);
    let requests: Vec<_> = gateway
        .provider
        .requests()
        .into_iter()
        .filter(|r| r.path.ends_with("/resets"))
        .collect();
    assert!(requests
        .iter()
        .all(|r| r.authorization.as_deref() == Some("Bearer cma-service")));
    assert_eq!(requests[1].body, requests[2].body);
    assert_eq!(requests[1].idempotency_key, requests[2].idempotency_key);
    assert_eq!(requests[1].idempotency_key.as_ref().unwrap().len(), 64);
    gateway.close().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn compiled_reset_scopes_keys_and_fingerprints_consumers_and_unicode_bytes() {
    let gateway = Gateway::new(FakeProviderConfig {
        inspection_agent: Some(("agt_deleted".into(), "deleted".into())),
        ..Default::default()
    })
    .await;
    gateway
        .provider
        .reset_controls()
        .enabled
        .store(true, Ordering::SeqCst);
    let channel = created(&gateway.client, &gateway.base).await;
    let id = channel["channel_id"].as_str().unwrap();
    let original = gateway.record(&channel).await;
    gateway.replace(&original, add_consumer).await;
    let before = gateway.record(&channel).await;
    let named_before = gateway
        .inspect(id, "repair-fixture", "csm_00000000000a")
        .await;
    let reset_body = body();
    let (one, two) = tokio::join!(
        gateway.reset(id, "same", &reset_body, 200),
        gateway.reset(id, "same", &reset_body, 200)
    );
    assert_eq!(one, two);
    assert_eq!(gateway.provider.reset_count(), 1);
    let mut null = body();
    null["consumer_id"] = Value::Null;
    assert_eq!(gateway.reset(id, "same", &null, 200).await, one);
    for (field, value) in [
        ("partition_key", json!("other")),
        ("expected_agent_id", json!("other")),
        ("consumer_id", json!("default")),
        ("consumer_id", json!("csm_00000000000a")),
    ] {
        let mut changed = body();
        changed[field] = value;
        assert_eq!(
            gateway.reset(id, "same", &changed, 409).await["detail"],
            "partition_reset_key_conflict"
        );
    }
    assert_eq!(
        gateway
            .inspect(id, "repair-fixture", "csm_00000000000a")
            .await,
        named_before
    );
    let mut named = body();
    named["consumer_id"] = "csm_00000000000a".into();
    let receipt = gateway.reset(id, "named", &named, 200).await;
    let expected = ConversationId::derive_for_consumer(
        &key_ring(1),
        &ChannelId::new(id).unwrap(),
        1,
        &ConsumerId::parse("csm_00000000000a").unwrap(),
        &cmaeg_core::ids::compact_partition_key("repair-fixture"),
    );
    assert_eq!(receipt["transport_conversation_id"], expected.as_str());
    let observed = gateway
        .provider
        .requests()
        .into_iter()
        .rfind(|r| r.path.ends_with("/resets"))
        .unwrap();
    let forwarded: Value = serde_json::from_str(&observed.body).unwrap();
    assert_eq!(
        forwarded["idempotency_binding_id"],
        channel["provider"]["binding_id"]
    );
    let mut unicode = body();
    unicode["partition_key"] = "é".repeat(4096).into();
    unicode["expected_agent_id"] = "é".repeat(64).into();
    gateway.reset(id, &"k".repeat(128), &unicode, 200).await;
    let second = created(&gateway.client, &gateway.base).await;
    gateway
        .reset(second["channel_id"].as_str().unwrap(), "same", &body(), 200)
        .await;
    assert_eq!(gateway.provider.reset_count(), 4);
    assert_eq!(gateway.record(&channel).await, before);
    gateway.close().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn compiled_reset_reports_committed_snapshot_and_refuses_changed_retry_authority() {
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let mut config = FakeProviderConfig::default();
    config.reset.gate = Some(gate.clone());
    let gateway = Gateway::new(config).await;
    let controls = gateway.provider.reset_controls();
    controls.enabled.store(true, Ordering::SeqCst);
    let channel = created(&gateway.client, &gateway.base).await;
    let id = channel["channel_id"].as_str().unwrap();
    gateway
        .replace(&gateway.record(&channel).await, add_consumer)
        .await;
    let original = gateway.record(&channel).await;
    let mut reset_body = body();
    reset_body["consumer_id"] = "csm_00000000000a".into();
    let pending = tokio::spawn(
        request(&gateway.client, &gateway.base, id)
            .header("Idempotency-Key", "snapshot")
            .json(&reset_body)
            .send(),
    );
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while gateway.provider.reset_count() != 1 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    gateway
        .records()
        .update_one(
            doc! {"_id":original.get_str("_id").unwrap()},
            doc! {"$set":{"version":2_i64}},
        )
        .await
        .unwrap();
    gate.add_permits(1);
    let committed = response(pending.await.unwrap().unwrap(), 200).await;
    assert_eq!(committed["reset"]["state"], "reserved");
    gateway.reset(id, "snapshot", &reset_body, 409).await;
    gateway
        .records()
        .replace_one(
            doc! {"_id":original.get_str("_id").unwrap()},
            original.clone(),
        )
        .await
        .unwrap();
    controls.lose_once.store(true, Ordering::SeqCst);
    gate.add_permits(1);
    gateway.reset(id, "uncertain", &reset_body, 503).await;
    let calls = gateway.provider.requests().len();
    for change in ["owner", "kind", "consumer", "provider", "default_binding"] {
        gateway
            .replace(&original, |definition| match change {
                "owner" => definition["owner"]["subject"] = "bob".into(),
                "kind" => definition["owner"]["kind"] = "organization".into(),
                "provider" => {
                    definition["routing"]["consumers"][0]["provider"]["binding_id"] =
                        "binding_changed".into()
                }
                "consumer" => definition["routing"]["consumers"][0]["pending"] = true.into(),
                _ => definition["provider"]["binding_id"] = "new_default_scope".into(),
            })
            .await;
        if matches!(change, "provider" | "default_binding") {
            gateway
                .records()
                .update_one(
                    doc! {"_id":original.get_str("_id").unwrap()},
                    doc! {"$set":{"version":2_i64}},
                )
                .await
                .unwrap();
        }
        gateway
            .reset(
                id,
                "uncertain",
                &reset_body,
                if change == "owner" || change == "kind" {
                    404
                } else if change == "consumer" {
                    400
                } else {
                    409
                },
            )
            .await;
    }
    assert_eq!(gateway.provider.requests().len(), calls);
    assert_eq!(gateway.provider.reset_count(), 2);
    gateway.close().await;
}

fn add_consumer(definition: &mut Value) {
    definition["routing"] = json!({"version":1,"default_consumer":{"name":"lead"},"default_targets":["default"],
            "consumers":[{"id":"csm_00000000000a","name":"reviewer","profile":definition["profile"],
                "profile_digest":definition["profile_digest"],"provider":{"slug":"cma_codex","binding_id":"binding_reviewer","level":"l1"},
                "agent_key_hash":cmaeg::channels::definition::hash_secret("nyx_reset_reviewer"),"created_at":"2026-01-01T00:00:00Z"}]});
    definition["secrets"]["consumer_keys"] = json!([["csm_00000000000a", "nyx_reset_reviewer"]]);
}
