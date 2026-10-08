//! The write gate through the public management API: no management path can
//! store a shape an older image cannot read, and every refusal keeps the
//! exact problem body callers received before readers widened.
use super::*;
use serde_json::Value;

const DUPLICATED: &str = "/sources/1: source variant is duplicated";

fn lark_relay() -> Value {
    json!({"type":"nyxid_relay","issuer":"https://nyx.example","key_id":"route-key",
        "route_ids":["route-2"],"platform":"lark","admission":{"type":"all"}})
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

struct Gateway {
    provider: FakeProvider,
    cma: FakeCma,
    nyxid: FakeNyxid,
    base: String,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
    client: Client,
}

impl Gateway {
    async fn start() -> Self {
        let provider = FakeProvider::start(FakeProviderConfig::default()).await;
        let cma = FakeCma::start(false).await;
        let nyxid = FakeNyxid::start().await;
        let (base, stop, task) = app(&provider, &cma, &nyxid).await;
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

    async fn update(&self, channel_id: &str, body: &Value) -> reqwest::Response {
        self.client
            .put(format!("{}/v1/channels/{channel_id}", self.base))
            .bearer_auth("token-alice")
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
