//! Routing writes, guarded exits and their observer/cache effects over HTTP.
use super::*;
use cmaeg::{
    auth::Creator,
    channels::{
        definition::channel_pointer, observer::ChannelIndexObserver, routing_policy, ChannelTarget,
    },
};
use cmaeg_core::{
    channel::{ChannelDefinition, Extensions},
    ids::{ChannelId, RecordId},
    profile::canonical_json,
    seal::{open_blob, seal_blob},
};
use serde_json::Value;
use std::sync::Mutex;

#[path = "routing_exits.rs"]
mod exits;
use super::routing_fixture as fixture;

#[derive(Default)]
struct Index(Mutex<Vec<(String, String, u64)>>);
impl ChannelIndexObserver for Index {
    fn changed(&self, record: &RecordId, owner: &str, version: u64) {
        self.0
            .lock()
            .unwrap()
            .push((record.to_string(), owner.into(), version));
    }
    fn removed(&self, _: &RecordId) {
        panic!("removing routing must retain the channel's index entry");
    }
}

struct Gateway {
    state: ManagementState,
    provider: FakeProvider,
    other: FakeProvider,
    cma: FakeCma,
    nyxid: FakeNyxid,
    index: Arc<Index>,
    base: String,
    client: Client,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}
impl Gateway {
    async fn start(enabled: bool, cap: u32, other_status: u16) -> Self {
        let provider = FakeProvider::start(Default::default()).await;
        let other = FakeProvider::start(FakeProviderConfig {
            unbind_status: std::collections::BTreeMap::from([("binding-nyx".into(), other_status)]),
            ..Default::default()
        })
        .await;
        let cma = FakeCma::start(false).await;
        let nyxid = FakeNyxid::start().await;
        let mut state = management_state(&provider, &cma, &nyxid, key_ring(1));
        let entries: Vec<Value> = [("cma_codex", provider.base_url()), ("nyxbot", other.base_url())]
            .into_iter().map(|(slug, base)| json!({
                "slug":slug,"base_url":base,"kind":"responses_http",
                "paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}",
                    "conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},
                "max_inflight":8
            })).collect();
        state.providers = Arc::new(
            ProviderClient::new(
                ProviderRegistry::parse(&serde_json::to_string(&entries).unwrap()).unwrap(),
                std::time::Duration::from_secs(1),
                std::time::Duration::from_secs(2),
                8,
            )
            .unwrap(),
        );
        state.routing.multi_consumer_enabled = enabled;
        state.routed_capacity.per_channel = cap;
        let index = Arc::new(Index::default());
        state.index = index.clone();
        let (base, stop, task) = serve_app(state.clone()).await;
        Self {
            state,
            provider,
            other,
            cma,
            nyxid,
            index,
            base,
            client: Client::new(),
            stop,
            task,
        }
    }
    fn seed(&self, edit: impl FnOnce(&mut Value)) -> (ChannelId, RecordId, ChannelDefinition) {
        let mut value = fixture::routed();
        value["record_id"] = uuid::Uuid::new_v4().to_string().into();
        value["owner"]["subject"] = "alice".into();
        value["profile"]["metadata"]["owner"]["subject"] = "alice".into();
        value["profile_digest"] = fixture::digest_of(&value["profile"]).into();
        for consumer in value["routing"]["consumers"].as_array_mut().unwrap() {
            consumer["profile"]["metadata"]["owner"]["subject"] = "alice".into();
            consumer["profile_digest"] = fixture::digest_of(&consumer["profile"]).into();
        }
        // A chat-only legacy source shape, with an inert custom partition key
        // to prove the exit removes it without changing any source.
        value["sources"] = json!([fixture::relay("lark", fixture::ROUTE)]);
        value["reply"]["sinks"] = json!([{"type":"source"}]);
        value["partition"]["key"] = "conversation".into();
        value["partition"]["custom_key"] = "event_subject".into();
        edit(&mut value);
        let definition = fixture::definition(value);
        definition.validate().unwrap();
        let blob = seal_blob(
            &key_ring(1),
            canonical_json(definition.to_blob_json()).as_bytes(),
        )
        .unwrap();
        self.cma.seed(definition.record_id.as_str(), "alice", blob);
        let id = channel_pointer(&self.cma.base_url, &definition.record_id)
            .unwrap()
            .seal_id(&key_ring(1))
            .unwrap();
        (id, definition.record_id.clone(), definition)
    }
    fn stored(&self, record: &RecordId) -> (ChannelDefinition, Vec<u8>, u64) {
        let record = self.cma.records()[record.as_str()].clone();
        let bytes = open_blob(&key_ring(1), &record.blob).unwrap().payload;
        (
            serde_json::from_slice(&bytes).unwrap(),
            bytes,
            record.version,
        )
    }
    async fn put(&self, id: &ChannelId, body: &Value) -> reqwest::Response {
        self.client
            .put(format!("{}/v1/channels/{id}/routing", self.base))
            .bearer_auth("token-alice")
            .json(body)
            .send()
            .await
            .unwrap()
    }
    async fn remove(&self, id: &ChannelId, version: u64, cascade: bool) -> reqwest::Response {
        self.client
            .delete(format!(
                "{}/v1/channels/{id}/routing?expected_version={version}&cascade={cascade}",
                self.base
            ))
            .bearer_auth("token-alice")
            .send()
            .await
            .unwrap()
    }
    fn calls(&self) -> usize {
        self.provider.requests().len() + self.other.requests().len()
    }
    async fn close(self) {
        self.stop.send(()).unwrap();
        self.task.await.unwrap();
        self.provider.close().await;
        self.other.close().await;
        self.cma.close().await;
        self.nyxid.close().await;
    }
}

fn body(version: u64) -> Value {
    json!({"expected_version": version,"default_consumer":{"name":"lead","label":"Lead"},
        "default_targets":["default"],"rules":[],"addressing":{"mentions":true,"command":false},
        "reply_label":"always","default_reply":{"type":"relay_conversation","source_id":"relay_lark","route_id":fixture::ROUTE},
        "agent_events":{"max_hops":2,"per_consumer_per_minute":10,"per_channel_per_minute":30}})
}
async fn problem(response: reqwest::Response, status: u16, code: &str, detail: &str) {
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let value: Value = response.json().await.unwrap();
    assert_eq!(value["code"], code);
    assert_eq!(value["detail"], detail);
}

#[tokio::test]
async fn routing_put_enforces_the_turn_cap_preserves_consumers_and_invalidates_every_alias() {
    let app = Gateway::start(true, 4, 204).await;
    let (id, record, before) = app.seed(|_| {});
    let alias = id
        .open(&key_ring(1))
        .unwrap()
        .payload
        .seal_id(&key_ring(1))
        .unwrap();
    for channel in [&id, &alias] {
        app.state.resolver.resolve(channel.as_str()).await.unwrap();
    }
    assert_eq!(app.state.resolver.cache_len(), 2);
    let original = app.cma.records()[record.as_str()].clone();
    let mut request = body(1);
    request["max_targets"] = 5.into();
    problem(
        app.put(&id, &request).await,
        400,
        "invalid_request",
        "/max_targets: max_targets exceeds the routed channel turn cap (4)",
    )
    .await;
    assert_eq!(app.cma.records()[record.as_str()].blob, original.blob);
    assert!(app.index.0.lock().unwrap().is_empty());
    assert_eq!(app.calls(), 0);
    for version in [1, 2] {
        request["expected_version"] = version.into();
        if version == 1 {
            request["max_targets"] = 4.into();
        } else {
            request.as_object_mut().unwrap().remove("max_targets");
        }
        let response = app.put(&id, &request).await;
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let value: Value = response.json().await.unwrap();
        assert_eq!(value["version"], version + 1);
        let (after, _, stored_version) = app.stored(&record);
        assert_eq!(stored_version, version + 1);
        assert_eq!(
            after.routing.as_ref().unwrap().consumers,
            before.routing.as_ref().unwrap().consumers
        );
        assert_eq!(after.secrets.consumer_keys, before.secrets.consumer_keys);
        assert_eq!(after.routing.as_ref().unwrap().max_targets, 4);
    }
    assert_eq!(app.state.resolver.cache_len(), 0);
    assert_eq!(
        app.state
            .resolver
            .resolve(alias.as_str())
            .await
            .unwrap()
            .version,
        3
    );
    assert_eq!(
        *app.index.0.lock().unwrap(),
        [
            (record.to_string(), "alice".into(), 2),
            (record.to_string(), "alice".into(), 3)
        ]
    );
    assert_eq!(app.calls(), 0);
    app.close().await;
}

#[tokio::test]
async fn first_routing_put_is_owner_checked_and_rejects_consumer_injection() {
    let app = Gateway::start(true, 4, 204).await;
    let (id, record, _) = app.seed(|value| {
        value.as_object_mut().unwrap().remove("routing");
        value["secrets"]
            .as_object_mut()
            .unwrap()
            .remove("consumer_keys");
        value["partition"]
            .as_object_mut()
            .unwrap()
            .remove("custom_key");
    });
    let target = ChannelTarget::Record(record.clone());
    let owner = |name: &str| Creator {
        subject: name.into(),
        token_hash: "fixture".into(),
        organization: false,
    };
    assert!(matches!(
        routing_policy::put(
            &app.state,
            &owner("bob"),
            &target,
            serde_json::from_value(body(1)).unwrap()
        )
        .await,
        Err(cmaeg::channels::management::ManagementError::NotFound)
    ));
    let mut injection = body(1);
    injection["consumers"] = json!([]);
    problem(
        app.put(&id, &injection).await,
        400,
        "invalid_request",
        "request body is invalid",
    )
    .await;
    let loaded = routing_policy::put(
        &app.state,
        &owner("alice"),
        &target,
        serde_json::from_value(body(1)).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(loaded.record.version, 2);
    assert!(loaded.definition.routing.unwrap().consumers.is_empty());
    problem(
        app.put(&id, &body(1)).await,
        409,
        "conflict",
        "expected version 1, actual Some(2)",
    )
    .await;
    assert_eq!(app.index.0.lock().unwrap().len(), 1);
    assert_eq!(app.calls(), 0);
    app.close().await;
}
