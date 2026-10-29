//! Owner history revocation survives rollout shutdown without new platform authority.
use super::*;
use cmaeg::{
    native_lark::{Client as LarkClient, Transport},
    net::exchange::{HttpReply, TransportError},
};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, Method, StatusCode, Url};
use serde_json::Value;
use std::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};

#[derive(Default)]
struct Platform {
    unavailable: AtomicBool,
    calls: AtomicUsize,
}
impl Transport for Platform {
    fn exchange(
        &self,
        _: Method,
        url: Url,
        _: HeaderMap,
        _: Vec<u8>,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.unavailable.load(Ordering::SeqCst) {
            return Box::pin(async { Err(TransportError::Unavailable) });
        }
        let body = match url.path() {
            "/open-apis/auth/v3/tenant_access_token/internal" => {
                json!({"code":0,"tenant_access_token":"fixture-native-access-token","expire":7200})
            }
            "/open-apis/bot/v3/info" => json!({"code":0,"bot":{"open_id":"ou_native_bot"}}),
            _ => panic!("unexpected platform operation"),
        };
        Box::pin(async move {
            Ok(HttpReply::new(
                StatusCode::OK,
                serde_json::to_vec(&body).unwrap(),
            ))
        })
    }
}
fn body(region: &str) -> Value {
    let mut value = create_body();
    value["record_id"] = uuid::Uuid::new_v4().to_string().into();
    value["sources"] = json!([{
        "type":"native_lark","separate_app":true,"conversation_history":true,
        "identity":{"region":region,"app_id":"cli_native_app","tenant_key":"native_tenant","bot_open_id":"ou_native_bot"},
        "admission":{"type":"all"}
    }]);
    value["credentials"]["native_lark"] = json!({
        "app_secret":"fixture-native-app-secret-12345",
        "encrypt_key":"fixture-native-encrypt-key-12345",
        "verification_token":"fixture-native-verification-12345"
    });
    value
}
fn state(
    provider: &FakeProvider,
    cma: &FakeCma,
    nyxid: &FakeNyxid,
) -> (ManagementState, Arc<Platform>) {
    let mut state = management_state(provider, cma, nyxid, key_ring(1));
    let registry = ProviderRegistry::parse(&json!([{
        "slug":"cma_codex","base_url":provider.base_url(),"kind":"responses_http",
        "paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}",
        "conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses",
        "recovery":{"response":"/api/v2/responses/{response_id}","events":"/api/v2/responses/{response_id}/events"}},"max_inflight":8
    }]).to_string()).unwrap();
    state.providers = Arc::new(
        ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(2), 8).unwrap(),
    );
    state.native_lark_writes_enabled = true;
    state.native_lark_admission_enabled = true;
    state.native_lark_conversation_history_enabled = true;
    state.multi_consumer_writes_enabled = true;
    let platform = Arc::new(Platform::default());
    state.native_lark = Some(Arc::new(
        LarkClient::production()
            .unwrap()
            .with_transport(platform.clone()),
    ));
    (state, platform)
}
async fn create(base: &str, input: &Value) -> Value {
    let response = Client::new()
        .post(format!("{base}/v1/channels"))
        .bearer_auth("token-alice")
        .json(input)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201, "{}", response.text().await.unwrap());
    response.json().await.unwrap()
}
fn opt_out(input: &Value, saved: &Value) -> Value {
    let mut request = input.clone();
    let object = request.as_object_mut().unwrap();
    object.remove("record_id");
    object.remove("credentials");
    request["expected_version"] = saved["version"].clone();
    request["sources"][0]
        .as_object_mut()
        .unwrap()
        .remove("conversation_history");
    request
}
async fn update(base: &str, saved: &Value, input: &Value) -> reqwest::Response {
    Client::new()
        .put(format!(
            "{base}/v1/channels/{}",
            saved["channel_id"].as_str().unwrap()
        ))
        .bearer_auth("token-alice")
        .json(input)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn native_lark_history_opt_out_persists_with_gates_off_and_platform_unavailable() {
    for region in ["lark", "feishu"] {
        let provider = FakeProvider::start(FakeProviderConfig::default()).await;
        let cma = FakeCma::start(false).await;
        let nyxid = FakeNyxid::start().await;
        let (enabled, platform) = state(&provider, &cma, &nyxid);
        let (base, stop, task) = serve_app(enabled.clone()).await;
        let input = body(region);
        let saved = create(&base, &input).await;
        assert_eq!(platform.calls.load(Ordering::SeqCst), 2);
        stop.send(()).unwrap();
        task.await.unwrap();

        let mut disabled = enabled;
        disabled.native_lark_writes_enabled = false;
        disabled.native_lark_admission_enabled = false;
        disabled.native_lark_intake_enabled = false;
        disabled.native_lark_conversation_history_enabled = false;
        disabled.multi_consumer_writes_enabled = false;
        platform.unavailable.store(true, Ordering::SeqCst);
        let (base, stop, task) = serve_app(disabled).await;
        let response = update(&base, &saved, &opt_out(&input, &saved)).await;
        assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
        let revoked: Value = response.json().await.unwrap();
        assert_eq!(platform.calls.load(Ordering::SeqCst), 2);
        for key in ["channel_id", "record_id"] {
            assert_eq!(revoked[key], saved[key]);
        }
        for key in ["binding_id", "slug", "level"] {
            assert_eq!(revoked["provider"][key], saved["provider"][key]);
        }
        assert_eq!(
            revoked["definition"]["routing"],
            saved["definition"]["routing"]
        );
        assert_eq!(
            revoked["definition"]["sources"],
            opt_out(&input, &saved)["sources"]
        );
        let records = cma.records();
        let blob = &records[input["record_id"].as_str().unwrap()].blob;
        let opened = cmaeg_core::seal::open_blob(&key_ring(1), blob).unwrap();
        let stored: Value = serde_json::from_slice(&opened.payload).unwrap();
        assert_eq!(
            stored["secrets"]["native_lark"],
            input["credentials"]["native_lark"]
        );
        assert!(stored["sources"][0].get("conversation_history").is_none());

        let before = provider.requests().len();
        for change in ["reenable", "identity", "admission", "credentials"] {
            let mut request = opt_out(&input, &revoked);
            match change {
                "reenable" => request["sources"][0]["conversation_history"] = true.into(),
                "identity" => {
                    request["sources"][0]["identity"]["bot_open_id"] = "ou_other_bot".into()
                }
                "admission" => {
                    request["sources"][0]["admission"] =
                        json!({"type":"allowlist","sender_ids":["ou_human"]})
                }
                _ => request["credentials"] = input["credentials"].clone(),
            }
            if change == "credentials" {
                request["credentials"]["native_lark"]["app_secret"] =
                    "fixture-changed-app-secret-12345".into();
            }
            let response = update(&base, &revoked, &request).await;
            assert_eq!(
                response.status(),
                400,
                "{change}: {}",
                response.text().await.unwrap()
            );
        }
        assert_eq!(provider.requests().len(), before);
        assert_eq!(platform.calls.load(Ordering::SeqCst), 2);
        stop.send(()).unwrap();
        task.await.unwrap();
    }
}

#[tokio::test]
async fn native_lark_history_opt_out_does_not_skip_changed_identity_or_credentials_verification() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (enabled, platform) = state(&provider, &cma, &nyxid);
    let (base, stop, task) = serve_app(enabled).await;
    let input = body("lark");
    let saved = create(&base, &input).await;
    let before = provider.requests().len();
    platform.unavailable.store(true, Ordering::SeqCst);
    for change in ["identity", "credentials"] {
        let mut request = opt_out(&input, &saved);
        if change == "identity" {
            request["sources"][0]["identity"]["tenant_key"] = "other_tenant".into();
        } else {
            request["credentials"] = input["credentials"].clone();
            request["credentials"]["native_lark"]["encrypt_key"] =
                "fixture-changed-encrypt-key-12345".into();
        }
        let response = update(&base, &saved, &request).await;
        assert_eq!(
            response.status(),
            400,
            "{change}: {}",
            response.text().await.unwrap()
        );
    }
    assert_eq!(platform.calls.load(Ordering::SeqCst), 4);
    assert_eq!(provider.requests().len(), before);
    assert_eq!(
        cma.records()[input["record_id"].as_str().unwrap()].version,
        saved["version"].as_u64().unwrap()
    );
    stop.send(()).unwrap();
    task.await.unwrap();
}
