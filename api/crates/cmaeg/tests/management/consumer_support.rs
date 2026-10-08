//! Shared real-HTTP harness for consumer management and cross-replica races.
use super::routing_fixture as fixture;
use super::*;
use cmaeg::{
    auth::Creator,
    channels::{definition::channel_pointer, observer::ChannelIndexObserver},
};
use cmaeg_core::{
    channel::ChannelDefinition,
    ids::{ChannelId, RecordId},
    profile::canonical_json,
    seal::{open_blob, seal_blob},
};
use serde_json::Value;
use std::sync::Mutex;

#[derive(Default)]
pub(super) struct Index {
    pub changed: Mutex<Vec<u64>>,
    pub removed: Mutex<Vec<RecordId>>,
}
impl ChannelIndexObserver for Index {
    fn changed(&self, _: &RecordId, _: &str, version: u64) {
        self.changed.lock().unwrap().push(version);
    }
    fn removed(&self, record: &RecordId) {
        self.removed.lock().unwrap().push(record.clone());
    }
}
pub(super) struct Gateway {
    pub state: ManagementState,
    pub provider: FakeProvider,
    pub other: FakeProvider,
    pub cma: FakeCma,
    nyxid: FakeNyxid,
    pub index: Arc<Index>,
    pub base: String,
    pub client: Client,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}
impl Gateway {
    pub async fn start(enabled: bool) -> Self {
        Self::configured(enabled, Default::default()).await
    }
    pub async fn configured(enabled: bool, config: FakeProviderConfig) -> Self {
        let provider = FakeProvider::start(config).await;
        let other = FakeProvider::start(Default::default()).await;
        let cma = FakeCma::start(false).await;
        let nyxid = FakeNyxid::start().await;
        let mut state = management_state(&provider, &cma, &nyxid, key_ring(1));
        let entries: Vec<Value> = [("cma_codex", provider.base_url()), ("nyxbot", other.base_url())]
            .into_iter().map(|(slug, base)| json!({"slug":slug,"base_url":base,"kind":"responses_http",
                "paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}",
                    "conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":8})).collect();
        state.providers = Arc::new(
            ProviderClient::new(
                ProviderRegistry::parse(&serde_json::to_string(&entries).unwrap()).unwrap(),
                std::time::Duration::from_secs(1),
                std::time::Duration::from_secs(5),
                8,
            )
            .unwrap(),
        );
        state.routing.multi_consumer_enabled = enabled;
        state.routing.non_human_providers.insert("nyxbot".into());
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
    pub fn seed(&self, edit: impl FnOnce(&mut Value)) -> (ChannelId, RecordId, ChannelDefinition) {
        let mut value = fixture::routed();
        value["record_id"] = uuid::Uuid::new_v4().to_string().into();
        value["owner"]["subject"] = "alice".into();
        value["profile"]["metadata"]["owner"]["subject"] = "alice".into();
        value["profile_digest"] = fixture::digest_of(&value["profile"]).into();
        value["routing"]["consumers"] = json!([]);
        value["routing"]["rules"] = json!([]);
        value["routing"]["default_targets"] = json!(["default"]);
        value["routing"]["default_consumer"]["publish"] = json!({"enabled":false,"targets":[]});
        value["secrets"]["consumer_keys"] = json!([]);
        value["sources"] = json!([fixture::relay("lark", fixture::ROUTE)]);
        value["reply"]["sinks"] = json!([{"type":"source"}]);
        value["partition"]["key"] = "conversation".into();
        edit(&mut value);
        let definition = fixture::definition(value);
        definition.validate().unwrap();
        self.cma.seed(
            definition.record_id.as_str(),
            "alice",
            seal_blob(
                &key_ring(1),
                canonical_json(definition.to_blob_json()).as_bytes(),
            )
            .unwrap(),
        );
        let id = channel_pointer(&self.cma.base_url, &definition.record_id)
            .unwrap()
            .seal_id(&key_ring(1))
            .unwrap();
        (id, definition.record_id.clone(), definition)
    }
    pub fn stored(&self, record: &RecordId) -> (ChannelDefinition, u64) {
        let stored = self.cma.records()[record.as_str()].clone();
        (
            serde_json::from_slice(&open_blob(&key_ring(1), &stored.blob).unwrap().payload)
                .unwrap(),
            stored.version,
        )
    }
    pub async fn replace(
        &self,
        record: &RecordId,
        edit: impl FnOnce(&mut ChannelDefinition),
    ) -> u64 {
        let (mut definition, version) = self.stored(record);
        edit(&mut definition);
        definition.validate().unwrap();
        self.state
            .records
            .put(
                record,
                &definition.owner.subject,
                &seal_blob(
                    &key_ring(1),
                    canonical_json(definition.to_blob_json()).as_bytes(),
                )
                .unwrap(),
                Some(version),
            )
            .await
            .unwrap()
    }
    pub async fn add(&self, id: &ChannelId, body: &Value) -> reqwest::Response {
        self.client
            .post(format!("{}/v1/channels/{id}/consumers", self.base))
            .bearer_auth("token-alice")
            .json(body)
            .send()
            .await
            .unwrap()
    }
    pub async fn update(&self, id: &ChannelId, consumer: &str, body: &Value) -> reqwest::Response {
        self.client
            .put(format!(
                "{}/v1/channels/{id}/consumers/{consumer}",
                self.base
            ))
            .bearer_auth("token-alice")
            .json(body)
            .send()
            .await
            .unwrap()
    }
    pub async fn remove(&self, id: &ChannelId, consumer: &str, version: u64) -> reqwest::Response {
        self.client
            .delete(format!(
                "{}/v1/channels/{id}/consumers/{consumer}?expected_version={version}",
                self.base
            ))
            .bearer_auth("token-alice")
            .send()
            .await
            .unwrap()
    }
    pub fn calls(&self) -> usize {
        self.provider.requests().len() + self.other.requests().len()
    }
    pub async fn close(self) {
        self.stop.send(()).unwrap();
        self.task.await.unwrap();
        self.provider.close().await;
        self.other.close().await;
        self.cma.close().await;
        self.nyxid.close().await;
    }
}
pub(super) fn body(version: u64, slug: &str, name: &str) -> Value {
    let mut profile = create_body()["profile"].clone();
    profile["metadata"]["provider"] = slug.into();
    json!({"expected_version":version,"name":name,"label":"Reviewer","provider":{"slug":slug,"target":{"type":"profile"}},
        "profile":profile,"credentials":{"agent_key":format!("fixture-key-{name}")},"mentionable":true,"non_human":null,
        "reply":{"auto_final":true,"destination":"origin"},"publish":{"enabled":true,"targets":["default"]}})
}
pub(super) fn creator() -> Creator {
    Creator {
        subject: "alice".into(),
        token_hash: "fixture".into(),
        organization: false,
    }
}
pub(super) async fn detail(response: reqwest::Response, status: u16) -> String {
    assert_eq!(response.status(), status);
    response.json::<Value>().await.unwrap()["detail"]
        .as_str()
        .unwrap()
        .into()
}
pub(super) async fn added(gateway: &Gateway, id: &ChannelId, version: u64, name: &str) -> Value {
    let response = gateway.add(id, &body(version, "cma_codex", name)).await;
    assert_eq!(response.status(), 201, "{}", response.text().await.unwrap());
    response.json().await.unwrap()
}
