//! Management of a custom source's attachments setting behind the media switch.
use super::*;
use serde_json::Value;

/// Fixed so the retained creation fingerprint below is reproducible.
const RECORD_ID: &str = "6f1c2a4e-3b5d-4c7e-9f80-1a2b3c4d5e6f";
/// The fingerprint this request produced before custom sources could carry
/// attachments. Requests without the setting must keep matching retained
/// reservations, or replaying a creation would answer 409.
const GOLDEN_FINGERPRINT: &str =
    "sha256:f8ad159588be6c89ffacaa9e985405ee43ec610e376c46d36ff51de88d071306";

fn custom_body() -> Value {
    let mut body = create_body();
    body["sources"] = json!([{"type":"custom","auth":{"type":"hmac","key_id":"h1"}}]);
    body["partition"]["key"] = "per_event".into();
    body
}

fn with_attachments(origins: Value) -> Value {
    let mut body = custom_body();
    body["sources"][0]["attachments"] = json!({"pointer":"/attachments","origins":origins});
    body
}

struct Gateway {
    provider: FakeProvider,
    cma: FakeCma,
    nyxid: FakeNyxid,
    client: Client,
}

impl Gateway {
    async fn start() -> Self {
        Self {
            provider: FakeProvider::start(FakeProviderConfig::default()).await,
            cma: FakeCma::start(false).await,
            nyxid: FakeNyxid::start().await,
            client: Client::new(),
        }
    }

    /// One replica over the shared fakes, with inbound media on or off.
    async fn replica(
        &self,
        media_enabled: bool,
    ) -> (String, oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
        let mut state = management_state(&self.provider, &self.cma, &self.nyxid, key_ring(1));
        state.media_enabled = media_enabled;
        serve_app(state).await
    }

    async fn create(&self, base: &str, body: &Value) -> reqwest::Response {
        self.client
            .post(format!("{base}/v1/channels"))
            .bearer_auth("token-alice")
            .json(body)
            .send()
            .await
            .unwrap()
    }

    async fn update(&self, base: &str, created: &Value, mut body: Value) -> reqwest::Response {
        body.as_object_mut().unwrap().remove("credentials");
        body["expected_version"] = created["version"].clone();
        self.client
            .put(format!(
                "{base}/v1/channels/{}",
                created["channel_id"].as_str().unwrap()
            ))
            .bearer_auth("token-alice")
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    async fn close(self) {
        self.provider.close().await;
        self.cma.close().await;
        self.nyxid.close().await;
    }
}

async fn refused(response: reqwest::Response, detail: &str) {
    assert_eq!(response.status(), 400);
    let problem: Value = response.json().await.unwrap();
    assert_eq!(problem["code"], "invalid_request");
    assert_eq!(problem["detail"], detail);
}

#[tokio::test]
async fn creating_with_attachments_is_refused_while_media_is_disabled() {
    let gateway = Gateway::start().await;
    let (base, stop, task) = gateway.replica(false).await;
    for reserved in [false, true] {
        let mut body = with_attachments(json!(["https://files.example"]));
        if reserved {
            // The recoverable creation flow refuses before reserving a record.
            body["record_id"] = RECORD_ID.into();
        }
        refused(
            gateway.create(&base, &body).await,
            "/sources/0/attachments: custom_attachments_disabled",
        )
        .await;
        assert!(gateway.provider.requests().is_empty());
        assert!(gateway.cma.records().is_empty());
    }
    stop.send(()).unwrap();
    task.await.unwrap();
    gateway.close().await;
}

#[tokio::test]
async fn creating_with_attachments_while_enabled_stores_and_returns_the_setting() {
    let gateway = Gateway::start().await;
    let (base, stop, task) = gateway.replica(true).await;
    for reserved in [false, true] {
        let mut body = with_attachments(json!(["https://files.example"]));
        if reserved {
            body["record_id"] = uuid::Uuid::new_v4().to_string().into();
        }
        let response = gateway.create(&base, &body).await;
        assert_eq!(response.status(), 201);
        let created: Value = response.json().await.unwrap();
        let expected = json!({"pointer":"/attachments","origins":["https://files.example"]});
        assert_eq!(created["definition"]["sources"][0]["attachments"], expected);
        let read: Value = gateway
            .client
            .get(format!(
                "{base}/v1/channels/{}",
                created["channel_id"].as_str().unwrap()
            ))
            .bearer_auth("token-alice")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(read["definition"]["sources"][0]["attachments"], expected);
    }
    let mut defaulted = custom_body();
    defaulted["sources"][0]["attachments"] = json!({});
    let response = gateway.create(&base, &defaulted).await;
    assert_eq!(response.status(), 201);
    let created: Value = response.json().await.unwrap();
    assert_eq!(
        created["definition"]["sources"][0]["attachments"],
        json!({"pointer":"/attachments","origins":[]})
    );
    stop.send(()).unwrap();
    task.await.unwrap();
    gateway.close().await;
}

#[tokio::test]
async fn updates_may_remove_but_not_add_the_setting_while_media_is_disabled() {
    let gateway = Gateway::start().await;
    let (enabled, stop_enabled, enabled_task) = gateway.replica(true).await;
    let (disabled, stop_disabled, disabled_task) = gateway.replica(false).await;

    let response = gateway.create(&disabled, &custom_body()).await;
    assert_eq!(response.status(), 201);
    let plain: Value = response.json().await.unwrap();
    let binds = gateway.provider.requests().len();
    refused(
        gateway
            .update(
                &disabled,
                &plain,
                with_attachments(json!(["https://files.example"])),
            )
            .await,
        "/sources/0/attachments: custom_attachments_disabled",
    )
    .await;
    assert_eq!(gateway.provider.requests().len(), binds);

    let response = gateway
        .create(
            &enabled,
            &with_attachments(json!(["https://files.example"])),
        )
        .await;
    assert_eq!(response.status(), 201);
    let configured: Value = response.json().await.unwrap();
    let response = gateway.update(&disabled, &configured, custom_body()).await;
    assert_eq!(response.status(), 200);
    let updated: Value = response.json().await.unwrap();
    assert!(updated["definition"]["sources"][0]
        .get("attachments")
        .is_none());

    stop_enabled.send(()).unwrap();
    enabled_task.await.unwrap();
    stop_disabled.send(()).unwrap();
    disabled_task.await.unwrap();
    gateway.close().await;
}

#[tokio::test]
async fn private_ip_literal_origins_are_refused_and_public_ones_admitted() {
    let gateway = Gateway::start().await;
    let (base, stop, task) = gateway.replica(true).await;
    for (position, origin) in [
        (0, "https://10.0.0.1"),
        (1, "https://[::1]"),
        (1, "https://169.254.169.254:8443"),
    ] {
        let origins = if position == 0 {
            json!([origin])
        } else {
            json!(["https://files.example", origin])
        };
        refused(
            gateway.create(&base, &with_attachments(origins)).await,
            &format!("/sources/0/attachments/origins/{position}: attachment_origin_not_public"),
        )
        .await;
    }
    assert!(gateway.cma.records().is_empty());
    let response = gateway
        .create(&base, &with_attachments(json!(["https://8.8.8.8"])))
        .await;
    assert_eq!(response.status(), 201);
    let response = gateway
        .create(&base, &with_attachments(json!(["https://files.example/"])))
        .await;
    assert_eq!(response.status(), 400);
    let problem: Value = response.json().await.unwrap();
    assert!(problem["detail"]
        .as_str()
        .unwrap()
        .starts_with("/sources/0/attachments/origins/0: "));
    stop.send(()).unwrap();
    task.await.unwrap();
    gateway.close().await;
}

#[tokio::test]
async fn a_request_without_attachments_keeps_its_creation_fingerprint() {
    let gateway = Gateway::start().await;
    let (base, stop, task) = gateway.replica(true).await;
    let mut body = custom_body();
    body["record_id"] = RECORD_ID.into();
    let response = gateway.create(&base, &body).await;
    assert_eq!(response.status(), 201);
    let created: Value = response.json().await.unwrap();
    assert_eq!(
        created["definition"]["creation"]["fingerprint"],
        GOLDEN_FINGERPRINT
    );
    assert!(created["definition"]["sources"][0]
        .get("attachments")
        .is_none());
    stop.send(()).unwrap();
    task.await.unwrap();
    gateway.close().await;
}
