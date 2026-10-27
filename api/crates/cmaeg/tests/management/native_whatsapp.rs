//! Native authoring proves Meta custody before a provider or record mutation.
use super::*;
use cmaeg::{
    net::exchange::{HttpReply, TransportError},
    whatsapp::{Client as GraphClient, Transport},
};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, Method, StatusCode, Url};
use serde_json::Value;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    time::Duration,
};

struct Graph {
    valid: bool,
    calls: AtomicUsize,
    methods: Mutex<Vec<Method>>,
}
impl Transport for Graph {
    fn exchange(
        &self,
        method: Method,
        url: Url,
        headers: HeaderMap,
        _: Vec<u8>,
        _: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.methods.lock().unwrap().push(method);
        assert!(headers["authorization"].is_sensitive());
        let body = if url.path().ends_with("/debug_token") {
            json!({"data":{"is_valid":true,"app_id":if self.valid {"12345"} else {"99999"},"scopes":["whatsapp_business_management","whatsapp_business_messaging"]}})
        } else {
            json!({"data":[{"id":"34567"}]})
        };
        Box::pin(async move {
            Ok(HttpReply::new(
                StatusCode::OK,
                serde_json::to_vec(&body).unwrap(),
            ))
        })
    }
}
fn body() -> Value {
    let mut value = create_body();
    value["record_id"] = uuid::Uuid::new_v4().to_string().into();
    value["sources"] = json!([{"type":"whatsapp","app_id":"12345","waba_id":"23456","phone_number_id":"34567","admission":{"type":"all"}}]);
    value["credentials"]["whatsapp"] = json!({"app_secret":"private-app-secret","access_token":"private-access-token","verify_token":"private-verify-token"});
    value
}
fn state(
    provider: &FakeProvider,
    cma: &FakeCma,
    nyxid: &FakeNyxid,
    valid: bool,
) -> (ManagementState, Arc<Graph>) {
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
    state.native_whatsapp_writes_enabled = true;
    state.native_whatsapp_admission_enabled = true;
    state.multi_consumer_writes_enabled = true;
    let graph = Arc::new(Graph {
        valid,
        calls: AtomicUsize::new(0),
        methods: Mutex::default(),
    });
    state.whatsapp = Some(Arc::new(
        GraphClient::production()
            .unwrap()
            .with_transport(graph.clone()),
    ));
    (state, graph)
}
async fn post(base: &str, body: &Value) -> reqwest::Response {
    Client::new()
        .post(format!("{base}/v1/channels"))
        .bearer_auth("token-alice")
        .json(body)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn native_whatsapp_management_refuses_missing_proof_or_rollout_requirements_before_writes() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    for case in 0..6 {
        let (mut state, graph) = state(&provider, &cma, &nyxid, case != 0);
        let mut input = body();
        match case {
            1 => state.native_whatsapp_writes_enabled = false,
            2 => state.native_whatsapp_admission_enabled = false,
            3 => {
                input.as_object_mut().unwrap().remove("record_id");
            }
            4 => input["partition"]["key"] = "thread".into(),
            5 => state.providers = management_state(&provider, &cma, &nyxid, key_ring(1)).providers,
            _ => {}
        }
        let (base, stop, task) = serve_app(state).await;
        let response = post(&base, &input).await;
        assert_eq!(
            response.status(),
            400,
            "case {case}: {}",
            response.text().await.unwrap()
        );
        assert!(cma.records().is_empty());
        assert!(provider.requests().is_empty());
        assert_eq!(graph.calls.load(Ordering::SeqCst), usize::from(case == 0));
        stop.send(()).unwrap();
        task.await.unwrap();
    }
}

#[tokio::test]
async fn native_whatsapp_management_seals_secrets_initializes_routing_and_replays_creation() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (state, graph) = state(&provider, &cma, &nyxid, true);
    let (base, stop, task) = serve_app(state).await;
    let input = body();
    let response = post(&base, &input).await;
    assert_eq!(response.status(), 201);
    let created: Value = response.json().await.unwrap();
    assert_eq!(
        created["definition"]["routing"]["default_targets"],
        json!(["default"])
    );
    assert!(created["endpoints"]["whatsapp_webhook_url"]
        .as_str()
        .unwrap()
        .contains("/v1/whatsapp/"));
    for secret in [
        "private-app-secret",
        "private-access-token",
        "private-verify-token",
    ] {
        assert!(!created.to_string().contains(secret));
    }
    assert_eq!(graph.calls.load(Ordering::SeqCst), 2);
    assert!(graph
        .methods
        .lock()
        .unwrap()
        .iter()
        .all(|method| *method == Method::GET));
    let records = cma.records();
    let saved = &records[input["record_id"].as_str().unwrap()];
    let opened = cmaeg_core::seal::open_blob(&key_ring(1), &saved.blob).unwrap();
    let private: Value = serde_json::from_slice(&opened.payload).unwrap();
    assert_eq!(
        private["secrets"]["whatsapp"],
        input["credentials"]["whatsapp"]
    );
    let writes = provider.requests().len();
    assert_eq!(post(&base, &input).await.status(), 201);
    assert_eq!(graph.calls.load(Ordering::SeqCst), 2);
    assert_eq!(provider.requests().len(), writes);
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn native_whatsapp_management_update_proves_new_source_and_initializes_missing_routing() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (state, graph) = state(&provider, &cma, &nyxid, true);
    let (base, stop, task) = serve_app(state).await;
    let old: Value = post(&base, &create_body()).await.json().await.unwrap();
    assert!(old["definition"].get("routing").is_none());
    let mut input = body();
    input.as_object_mut().unwrap().remove("record_id");
    input["expected_version"] = old["version"].clone();
    let response = Client::new()
        .put(format!(
            "{base}/v1/channels/{}",
            old["channel_id"].as_str().unwrap()
        ))
        .bearer_auth("token-alice")
        .json(&input)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let saved: Value = response.json().await.unwrap();
    assert_eq!(
        saved["definition"]["routing"]["default_targets"],
        json!(["default"])
    );
    assert_eq!(graph.calls.load(Ordering::SeqCst), 2);
    assert!(!saved.to_string().contains("private-access-token"));
    stop.send(()).unwrap();
    task.await.unwrap();
}
