use super::*;
use crate::hosted::{
    AgentSandboxBackend, HostedAgentOpenSettings, HostedBackend, HostedControlTransport,
};
use crate::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame};
use crate::{AgentLaunch, ConversationChannel, ConversationUpdate};

#[derive(Default)]
struct Transport(Mutex<Vec<RuntimeHandle>>);

#[async_trait::async_trait]
impl HostedControlTransport for Transport {
    async fn bind_runtime(&self, handle: &RuntimeHandle, _: &str) -> Result<()> {
        self.0.lock().unwrap().push(handle.clone());
        Ok(())
    }
    async fn exchange(&self, _: &RuntimeHandle, _: RequestFrame) -> Result<ResponseFrame> {
        panic!("no runtime exchange during provisioning");
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("no Agent admission during provisioning");
    }
    async fn retire(&self, _: &RuntimeHandle) -> Result<()> {
        panic!("no retirement during provisioning");
    }
}

#[tokio::test(start_paused = true)]
async fn startup_breaker_preserves_cold_create_delivery_and_read_timing() {
    let mut traces = Vec::new();
    for enabled in [false, true] {
        let (mut request, _) = super::object();
        request.metadata.insert(
            "cma/runtime-contract".into(),
            crate::RUNTIME_CONTRACT_VERSION.into(),
        );
        request.metadata.insert(
            "cma/image-sha256".into(),
            crate::hosted::image_sha256(&request.image.uri),
        );
        request.metadata.insert(
            "cma/spec-sha256".into(),
            crate::hosted::provider_request_fingerprint(&request).unwrap(),
        );
        let (expected, mut object, mut pods) = pod_read_tests::ready();
        let signer = AgentSandboxClient::new(
            kube::Client::try_from(kube::Config::new("http://127.0.0.1:1".parse().unwrap()))
                .unwrap(),
            super::super::tests::config(),
            "fixture",
        )
        .unwrap();
        let desired = model::desired(
            &signer.config,
            &request,
            &signer.control_token(&model::name(&request).unwrap()),
        )
        .unwrap();
        object["metadata"]["annotations"] =
            serde_json::to_value(desired.metadata.annotations).unwrap();
        pods["items"][0]["status"]["containerStatuses"][0]["containerID"] =
            json!(format!("containerd://{}", "a".repeat(64)));
        pods["items"][0]["status"]["containerStatuses"][0]["restartCount"] = json!(0);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let observed = calls.clone();
        let mut created = false;
        let service = tower::service_fn(move |request: http::Request<kube::client::Body>| {
            let path = request.uri().path();
            observed
                .lock()
                .unwrap()
                .push(format!("{} {path}", request.method()));
            let (status, body) = if request.method() == http::Method::POST {
                assert!(path.ends_with("/sandboxes"));
                assert!(!created);
                created = true;
                (201, object.clone())
            } else {
                assert_eq!(request.method(), http::Method::GET);
                if path.ends_with("/pods") {
                    (200, pods.clone())
                } else {
                    assert!(path.contains("/sandboxes/"));
                    if created {
                        (200, object.clone())
                    } else {
                        (404, missing())
                    }
                }
            };
            async move {
                Ok::<_, std::convert::Infallible>(
                    http::Response::builder()
                        .status(status)
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(kube::client::Body::from(serde_json::to_vec(&body).unwrap()))
                        .unwrap(),
                )
            }
        });
        let mut config = super::super::tests::config();
        config.runtime_startup_breaker_enabled = enabled;
        let client =
            AgentSandboxClient::new(kube::Client::new(service, "cma-test"), config, "fixture")
                .unwrap();
        let transport = Arc::new(Transport::default());
        let backend = AgentSandboxBackend::new(client, transport.clone());
        let started = tokio::time::Instant::now();
        assert_eq!(backend.provision(request).await.unwrap(), expected);
        assert_eq!(started.elapsed(), std::time::Duration::ZERO);
        assert_eq!(*transport.0.lock().unwrap(), [expected]);
        // Cold creation embeds the environment in the initial template and
        // does not issue a separate pooled bootstrap delivery.
        let mut calls = calls.lock().unwrap().clone();
        calls.sort();
        assert_eq!(
            calls
                .iter()
                .filter(|call| call.starts_with("POST "))
                .count(),
            1
        );
        traces.push(calls);
    }
    assert_eq!(traces[0], traces[1]);
}
