//! Public node operations preserve owner, consumer and durable record boundaries.
use super::*;
use cmaeg::channels::definition::{channel_pointer, hash_secret};
use cmaeg_core::{
    channel::ChannelDefinition,
    ids::RecordId,
    profile::{canonical_json, EventWorkerProfile},
    seal::{open_blob, seal_blob},
};
use serde_json::Value;

const REVIEWER: &str = "csm_00000000000a";
const WRITER: &str = "csm_00000000000b";
const REVIEWER_KEY: &str = "fixture-reviewer-credential";
const WRITER_KEY: &str = "fixture-writer-credential";

struct Nodes {
    provider: FakeProvider,
    cma: FakeCma,
    nyxid: FakeNyxid,
    base: String,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
    client: Client,
    channel_id: String,
    record_id: String,
}
impl Nodes {
    async fn start(writes: bool) -> Self {
        let provider = FakeProvider::start(FakeProviderConfig::default()).await;
        let cma = FakeCma::start(false).await;
        let nyxid = FakeNyxid::start().await;
        let mut state = management_state(&provider, &cma, &nyxid, key_ring(1));
        state.multi_consumer_writes_enabled = writes;
        let (channel_id, record_id) = seed(&cma);
        let (base, stop, task) = serve_app(state).await;
        Self {
            provider,
            cma,
            nyxid,
            base,
            stop,
            task,
            client: Client::new(),
            channel_id,
            record_id,
        }
    }
    fn url(&self, suffix: &str) -> String {
        format!("{}/v1/channels/{}{suffix}", self.base, self.channel_id)
    }
    fn stored(&self) -> ChannelDefinition {
        let record = &self.cma.records()[&self.record_id];
        let opened = open_blob(&key_ring(1), &record.blob).unwrap().payload;
        serde_json::from_slice(&opened).unwrap()
    }
    fn routing_request(&self, version: u64) -> Value {
        let mut routing = serde_json::to_value(self.stored().routing.unwrap()).unwrap();
        for consumer in routing["consumers"].as_array_mut().unwrap() {
            for key in [
                "profile",
                "profile_digest",
                "provider",
                "agent_key_hash",
                "pending",
                "created_at",
            ] {
                consumer.as_object_mut().unwrap().remove(key);
            }
        }
        json!({"expected_version": version, "routing": routing})
    }
    async fn update(&self, token: &str, body: &Value) -> reqwest::Response {
        self.client
            .put(self.url("/routing"))
            .bearer_auth(token)
            .header("Idempotency-Key", "routing-fixture-original-request")
            .json(body)
            .send()
            .await
            .unwrap()
    }
    async fn close(self) {
        self.stop.send(()).unwrap();
        self.task.await.unwrap();
        self.provider.close().await;
        self.cma.close().await;
        self.nyxid.close().await;
    }
}
fn seed(cma: &FakeCma) -> (String, String) {
    let record_id = uuid::Uuid::new_v4().to_string();
    let profile = create_body()["profile"].clone();
    let digest = serde_json::from_value::<EventWorkerProfile>(profile.clone())
        .unwrap()
        .digest();
    let consumer = |id: &str, name: &str, credential: &str| {
        json!({
            "id": id, "name": name, "label": name, "profile": profile,
            "profile_digest": digest,
            "provider": {"slug": "cma_codex", "binding_id": format!("binding_{name}"), "level": "l1"},
            "agent_key_hash": hash_secret(credential), "created_at": "2026-10-01T00:00:00Z"
        })
    };
    let definition: ChannelDefinition = serde_json::from_value(json!({
        "schema_version": 1, "record_id": record_id, "name": "node fixture",
        "owner": {"kind": "human", "subject": "alice"}, "profile": profile,
        "profile_digest": digest,
        "provider": {"slug": "cma_codex", "binding_id": "binding_default", "level": "l1"},
        "sources": [{"type": "nyxid_relay", "platform": "telegram", "issuer": "https://nyx.example",
            "jwks_url": "https://nyx.example/.well-known/jwks.json", "key_id": "key-id",
            "route_ids": ["route-1"], "admission": {"type": "all"}}],
        "partition": {"version": 1, "key": "conversation_and_sender"},
        "reply": {"auto_final": true, "ack_after_ms": 1500, "sinks": [{"type": "source"}]},
        "secrets": {"agent_key_hash": hash_secret("nyx_agent_secret"),
            "provider_credential": "nyx_agent_secret", "channel_key": "nyx_route_secret",
            "hmac_secrets": [["h1", base64::engine::general_purpose::STANDARD.encode(b"node-secret")]],
            "consumer_keys": [[REVIEWER, REVIEWER_KEY], [WRITER, WRITER_KEY]]},
        "routing": {"version": 1, "default_consumer": {"name": "default"},
            "consumers": [consumer(REVIEWER, "reviewer", REVIEWER_KEY), consumer(WRITER, "writer", WRITER_KEY)],
            "default_targets": ["default"], "max_targets": 4},
        "created_at": "2026-10-01T00:00:00Z", "version": 1
    })).unwrap();
    definition.validate().unwrap();
    let ring = key_ring(1);
    let blob = seal_blob(&ring, canonical_json(definition.to_blob_json()).as_bytes()).unwrap();
    cma.seed(&record_id, "alice", blob);
    let id = channel_pointer(&cma.base_url, &RecordId::new(record_id.clone()).unwrap())
        .unwrap()
        .seal_id(&ring)
        .unwrap();
    (id.to_string(), record_id)
}
async fn assert_status(response: reqwest::Response, status: u16) -> Value {
    let actual = response.status();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body: Value = response.json().await.unwrap();
    assert_eq!(actual, status, "{body}");
    body
}

#[tokio::test]
async fn producer_check_is_owner_fenced_configuration_only_and_has_no_provider_side_effects() {
    let fixture = Nodes::start(true).await;
    let before = fixture.cma.records()[&fixture.record_id].clone();
    for (token, index, status) in [("token-bob", 0, 404), ("token-alice", 99, 404)] {
        assert_status(
            fixture
                .client
                .post(fixture.url(&format!("/sources/{index}/test")))
                .bearer_auth(token)
                .send()
                .await
                .unwrap(),
            status,
        )
        .await;
    }
    let body = assert_status(
        fixture
            .client
            .post(fixture.url("/sources/0/test"))
            .bearer_auth("token-alice")
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(body["source_index"], 0);
    assert_eq!(body["platform"], "telegram");
    assert_eq!(body["scope"], "configuration_only");
    assert_eq!(body["external_delivery_tested"], false);
    let checks = body["checks"].as_array().unwrap();
    assert!(checks
        .iter()
        .any(|check| check["id"] == "credential" && check["status"] == "passed"));
    assert!(checks
        .iter()
        .any(|check| check["id"] == "external_delivery" && check["status"] == "unavailable"));
    for secret in [
        "nyx_agent_secret",
        "nyx_route_secret",
        REVIEWER_KEY,
        WRITER_KEY,
    ] {
        assert!(!body.to_string().contains(secret));
    }
    assert!(fixture.provider.requests().is_empty());
    let after = &fixture.cma.records()[&fixture.record_id];
    assert_eq!(after.version, before.version);
    assert_eq!(after.blob, before.blob);
    fixture.close().await;
}

#[tokio::test]
async fn consumer_test_uses_only_selected_binding_credential_and_isolated_conversation() {
    let fixture = Nodes::start(true).await;
    for (token, consumer, status) in [
        ("token-bob", REVIEWER, 404),
        ("token-alice", "csm_00000000000c", 404),
    ] {
        assert_status(
            fixture
                .client
                .post(fixture.url(&format!("/consumers/{consumer}/test")))
                .bearer_auth(token)
                .send()
                .await
                .unwrap(),
            status,
        )
        .await;
    }
    assert!(fixture.provider.requests().is_empty());
    let body = assert_status(
        fixture
            .client
            .post(fixture.url(&format!("/consumers/{REVIEWER}/test")))
            .bearer_auth("token-alice")
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(body["first_text"], "hello");
    let selected = fixture.provider.requests();
    assert_eq!(selected.len(), 2);
    assert!(
        selected
            .iter()
            .all(|request| request.authorization.as_deref()
                == Some(&format!("Bearer {REVIEWER_KEY}")))
    );
    let ensure = selected
        .iter()
        .find(|request| request.method == "PUT")
        .unwrap();
    assert!(ensure
        .path
        .contains("/bindings/binding_reviewer/conversations/"));
    assert!(!selected
        .iter()
        .any(|request| request.body.contains(WRITER_KEY)));
    assert_status(
        fixture
            .client
            .post(fixture.url("/consumers/default/test"))
            .bearer_auth("token-alice")
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    let requests = fixture.provider.requests();
    let default = requests
        .iter()
        .find(|request| {
            request
                .path
                .contains("/bindings/binding_default/conversations/")
        })
        .unwrap();
    assert_eq!(
        default.authorization.as_deref(),
        Some("Bearer nyx_agent_secret")
    );
    assert_ne!(
        ensure.path.rsplit('/').next(),
        default.path.rsplit('/').next()
    );
    assert_eq!(fixture.cma.records()[&fixture.record_id].version, 1);
    fixture.close().await;
}

#[tokio::test]
async fn routing_edit_preserves_secrets_profiles_binding_identities_and_stale_cas() {
    let fixture = Nodes::start(true).await;
    let before = fixture.stored();
    let blob = fixture.cma.records()[&fixture.record_id].blob.clone();
    let mut request = fixture.routing_request(1);
    request["routing"]["max_targets"] = 2.into();
    request["routing"]["consumers"][0]["label"] = "Updated reviewer".into();
    assert_status(fixture.update("token-bob", &request).await, 404).await;
    assert_eq!(fixture.cma.records()[&fixture.record_id].blob, blob);
    let body = assert_status(fixture.update("token-alice", &request).await, 200).await;
    assert_eq!(body["version"], 2);
    let after = fixture.stored();
    assert_eq!(after.provider, before.provider);
    assert_eq!(after.profile, before.profile);
    assert_eq!(
        after.to_blob_json()["secrets"],
        before.to_blob_json()["secrets"]
    );
    let old = before.routing.as_ref().unwrap();
    let new = after.routing.as_ref().unwrap();
    for (old, new) in old.consumers.iter().zip(&new.consumers) {
        assert_eq!(new.id, old.id);
        assert_eq!(new.provider, old.provider);
        assert_eq!(new.profile, old.profile);
        assert_eq!(new.profile_digest, old.profile_digest);
        assert_eq!(new.agent_key_hash, old.agent_key_hash);
        assert_eq!(new.created_at, old.created_at);
    }
    assert_eq!(new.consumers[0].label.as_deref(), Some("Updated reviewer"));
    assert!(fixture.provider.requests().is_empty());
    for secret in [
        "nyx_agent_secret",
        "nyx_route_secret",
        REVIEWER_KEY,
        WRITER_KEY,
    ] {
        assert!(!body.to_string().contains(secret));
    }
    let committed = fixture.cma.records()[&fixture.record_id].clone();
    request["routing"]["max_targets"] = 3.into();
    assert_status(fixture.update("token-alice", &request).await, 409).await;
    assert_eq!(
        fixture.cma.records()[&fixture.record_id].version,
        committed.version
    );
    assert_eq!(
        fixture.cma.records()[&fixture.record_id].blob,
        committed.blob
    );
    fixture.close().await;
}

#[tokio::test]
async fn immediately_committed_identical_routing_replay_confirms_without_writing_again() {
    let fixture = Nodes::start(true).await;
    let mut request = fixture.routing_request(1);
    request["routing"]["max_targets"] = 2.into();
    assert_status(fixture.update("token-alice", &request).await, 200).await;
    let committed = fixture.cma.records()[&fixture.record_id].clone();
    let replay = assert_status(fixture.update("token-alice", &request).await, 200).await;
    assert_eq!(replay["version"], 2);
    assert_eq!(
        fixture.cma.records()[&fixture.record_id].version,
        committed.version
    );
    assert_eq!(
        fixture.cma.records()[&fixture.record_id].blob,
        committed.blob
    );
    let mut newer = request.clone();
    newer["expected_version"] = 2.into();
    assert_status(fixture.update("token-alice", &newer).await, 200).await;
    assert_status(fixture.update("token-alice", &request).await, 409).await;
    assert_eq!(fixture.cma.records()[&fixture.record_id].version, 3);
    assert!(fixture.provider.requests().is_empty());
    fixture.close().await;
}

#[tokio::test]
async fn routing_policy_cannot_add_remove_reorder_or_replace_consumer_binding_and_credentials() {
    let fixture = Nodes::start(true).await;
    let original = fixture.routing_request(1);
    let before = fixture.cma.records()[&fixture.record_id].clone();
    for mutation in [
        "remove",
        "append",
        "reorder",
        "provider",
        "agent_key_hash",
        "profile",
    ] {
        let mut request = original.clone();
        let consumers = request["routing"]["consumers"].as_array_mut().unwrap();
        match mutation {
            "remove" => {
                consumers.pop();
            }
            "append" => {
                let mut added = consumers[0].clone();
                added["id"] = "csm_00000000000c".into();
                consumers.push(added);
            }
            "reorder" => consumers.swap(0, 1),
            key => consumers[0][key] = json!({"forged": true}),
        }
        assert_status(fixture.update("token-alice", &request).await, 400).await;
        assert_eq!(
            fixture.cma.records()[&fixture.record_id].version,
            before.version
        );
        assert_eq!(fixture.cma.records()[&fixture.record_id].blob, before.blob);
    }
    assert!(fixture.provider.requests().is_empty());
    fixture.close().await;
}

#[tokio::test]
async fn disabled_routing_write_gate_retains_existing_policy_without_allowing_changes() {
    let fixture = Nodes::start(false).await;
    let before = fixture.cma.records()[&fixture.record_id].clone();
    let mut request = fixture.routing_request(1);
    request["routing"]["max_targets"] = 2.into();
    let body = assert_status(fixture.update("token-alice", &request).await, 400).await;
    assert!(body["detail"]
        .as_str()
        .unwrap()
        .contains("multi_consumer_disabled"));
    assert_eq!(fixture.cma.records()[&fixture.record_id].blob, before.blob);
    assert_eq!(
        fixture.cma.records()[&fixture.record_id].version,
        before.version
    );
    assert!(fixture.provider.requests().is_empty());
    fixture.close().await;
}
