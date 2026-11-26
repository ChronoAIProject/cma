use super::*;
use cmaeg::channels::{
    definition::build_definition, observer::ChannelIndexObserver, write_gate::WriteFlags,
};
use cmaeg_core::{channel::ConformanceLevel, ids::BindingId};

#[derive(Default)]
pub(super) struct Observations(pub AtomicUsize);
impl ChannelIndexObserver for Observations {
    fn changed(&self, _: &RecordId, _: &str, _: u64) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    fn removed(&self, _: &RecordId) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    fn observed(&self, _: &RecordId, _: &ChannelId) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

pub(super) struct Fixture {
    pub provider: FakeProvider,
    pub cma: FakeCma,
    pub nyxid: FakeNyxid,
    pub observations: Arc<Observations>,
}
impl Fixture {
    pub async fn new() -> Self {
        Self {
            provider: FakeProvider::start(FakeProviderConfig::default()).await,
            cma: FakeCma::start(false).await,
            nyxid: FakeNyxid::start().await,
            observations: Arc::new(Observations::default()),
        }
    }
    pub fn state(&self) -> ManagementState {
        let mut state = management_state(&self.provider, &self.cma, &self.nyxid, key_ring(1));
        state.index = self.observations.clone();
        state
    }
    pub fn seed(&self, definition: &ChannelDefinition) {
        self.cma.seed(
            definition.record_id.as_str(),
            &definition.owner.subject,
            blob(definition),
        );
    }
    pub fn assert_no_effects(&self) {
        assert_eq!(self.cma.writes.load(Ordering::SeqCst), 0);
        assert_eq!(self.observations.0.load(Ordering::SeqCst), 0);
        assert!(self.provider.requests().is_empty());
        assert!(self.provider.log().is_empty());
    }
    pub async fn close(self) {
        self.assert_no_effects();
        self.provider.close().await;
        self.cma.close().await;
        self.nyxid.close().await;
    }
}

pub(super) fn definition() -> ChannelDefinition {
    definition_from(create_body())
}
pub(super) fn definition_from(body: Value) -> ChannelDefinition {
    build_definition(
        &serde_json::from_value(body.clone()).unwrap(),
        body["profile"]["metadata"]["owner"]["subject"]
            .as_str()
            .unwrap(),
        new_id(),
        BindingId::new("synthetic-binding").unwrap(),
        ConformanceLevel::L1,
        WriteFlags {
            native_lark: true,
            native_whatsapp: true,
            ..WriteFlags::default()
        },
    )
    .unwrap()
}
pub(super) fn new_id() -> RecordId {
    RecordId::new(uuid::Uuid::new_v4().to_string()).unwrap()
}
pub(super) fn blob(definition: &ChannelDefinition) -> Vec<u8> {
    sealed_json(&definition.to_blob_json())
}
pub(super) fn sealed_json(value: &Value) -> Vec<u8> {
    cmaeg_core::seal::seal_blob(&key_ring(1), &serde_json::to_vec(value).unwrap()).unwrap()
}
pub(super) fn saved_id(origin: &str, record: &RecordId, key: u8) -> ChannelId {
    cmaeg_core::seal::ChannelPointer {
        v: 1,
        record_url: format!("{origin}/api/v1/gateway/channel-records/{record}"),
        record_id: record.clone(),
    }
    .seal_id(&key_ring(key))
    .unwrap()
}
pub(super) fn creation(id: ChannelId, ready: bool) -> ChannelCreation {
    ChannelCreation {
        channel_id: id,
        ready,
        fingerprint: "private-creation-fingerprint".into(),
    }
}
pub(super) fn url(base: &str, record: &RecordId) -> String {
    format!("{base}/v1/channel-records/{record}/management")
}
pub(super) async fn get(base: &str, record: &RecordId) -> reqwest::Response {
    Client::new()
        .get(url(base, record))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap()
}
pub(super) fn headers(response: &reqwest::Response, status: u16) {
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["cmaeg-protocol-version"], "1");
}
pub(super) async fn read(response: reqwest::Response) -> Value {
    headers(&response, 200);
    let text = response.text().await.unwrap();
    super::super::binary_support::assert_no_secrets(&text);
    assert!(!text.contains("private-"));
    serde_json::from_str(&text).unwrap()
}
