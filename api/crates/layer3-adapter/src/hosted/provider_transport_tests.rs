use std::collections::BTreeMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::Message;

use super::*;
use crate::hosted::{
    CheckpointArchiveDownload, CheckpointArchiveReceipt, CheckpointArchiveUpload,
    HostedControlTransport,
};
use crate::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, ResponseFrame};
use crate::{AgentLaunch, AgentSlot, ResourceReading};

#[path = "provider_transport_agent_sandbox_tests.rs"]
mod agent_sandbox;
#[path = "provider_transport_checkpoint_tests.rs"]
mod checkpoint;
#[path = "provider_transport_history_tests.rs"]
mod history;
#[path = "provider_transport_restore_tests.rs"]
mod restore;
#[path = "provider_transport_retirement_tests.rs"]
mod retirement;

struct NullStore;

#[async_trait::async_trait]
impl CheckpointArchiveStore for NullStore {
    async fn upload(
        &self,
        _archive: CheckpointArchiveUpload,
    ) -> crate::Result<CheckpointArchiveReceipt> {
        Err(AdapterError::Unavailable("not used by this test".into()))
    }

    async fn download(&self, _object_reference: &str) -> crate::Result<CheckpointArchiveDownload> {
        Err(AdapterError::Unavailable("not used by this test".into()))
    }
}

#[derive(Default)]
struct RecordingFactory {
    sandboxes: StdMutex<Vec<String>>,
}

impl CheckpointArchiveStoreFactory for RecordingFactory {
    fn for_sandbox(&self, sandbox_id: &str) -> crate::Result<Arc<dyn CheckpointArchiveStore>> {
        self.sandboxes
            .lock()
            .expect("factory record lock")
            .push(sandbox_id.into());
        Ok(Arc::new(NullStore))
    }
}

async fn listener() -> tokio::net::TcpListener {
    tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("HTTP fixture binds")
}

fn http_response(status: &str, headers: &[(&str, String)], body: &[u8]) -> Vec<u8> {
    let mut response = format!("HTTP/1.1 {status}\r\nConnection: close\r\n");
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str("\r\n");
    let mut response = response.into_bytes();
    response.extend_from_slice(body);
    response
}

fn json_response(value: &impl serde::Serialize) -> Vec<u8> {
    let body = serde_json::to_vec(value).expect("fixture JSON serializes");
    http_response(
        "200 OK",
        &[
            ("Content-Type", "application/json".into()),
            ("Content-Length", body.len().to_string()),
        ],
        &body,
    )
}

fn spawn_http(
    listener: tokio::net::TcpListener,
    response: Vec<u8>,
) -> tokio::task::JoinHandle<Vec<u8>> {
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("HTTP fixture accepts");
        let request = read_request(&mut stream).await;
        stream
            .write_all(&response)
            .await
            .expect("HTTP fixture writes");
        stream.shutdown().await.expect("HTTP fixture closes");
        request
    })
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 1_024];
        let read = stream.read(&mut chunk).await.expect("HTTP fixture reads");
        assert!(read > 0, "client closed before headers completed");
        request.extend_from_slice(&chunk[..read]);
        if let Some(index) = request.windows(4).position(|part| part == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&request[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().expect("content length"))
        })
        .unwrap_or_default();
    while request.len() < header_end + content_length {
        let mut chunk = [0_u8; 1_024];
        let read = stream
            .read(&mut chunk)
            .await
            .expect("HTTP fixture reads body");
        assert!(read > 0, "client closed before body completed");
        request.extend_from_slice(&chunk[..read]);
    }
    request
}

async fn transport_fixture(
    runtime_response: Vec<u8>,
) -> (
    ProviderHostedControlTransport,
    tokio::task::JoinHandle<Vec<u8>>,
    tokio::task::JoinHandle<Vec<u8>>,
) {
    let runtime = listener().await;
    let runtime_address = runtime.local_addr().expect("runtime address");
    let provider = listener().await;
    let provider_address = provider.local_addr().expect("provider address");
    let endpoint = serde_json::json!({
        "endpoint": format!("{runtime_address}/proxy/3001"),
    });
    let provider_task = spawn_http(provider, json_response(&endpoint));
    let runtime_task = spawn_http(runtime, runtime_response);
    let factory: Arc<dyn CheckpointArchiveStoreFactory> = Arc::new(RecordingFactory::default());
    let transport = ProviderHostedControlTransport::new(
        SandboxClient::new(format!("http://{provider_address}"), "provider-secret"),
        factory,
    )
    .expect("transport builds");
    (transport, provider_task, runtime_task)
}

fn request_text(request: Vec<u8>) -> String {
    String::from_utf8(request).expect("fixture request is text")
}

#[tokio::test]
async fn exchange_uses_provider_proxy_prefix_and_header_authentication() {
    let response = ResponseFrame::Resources(ResourceReading::default());
    let (transport, provider, runtime) = transport_fixture(json_response(&response)).await;

    let actual = transport
        .exchange(&RuntimeHandle::new("runtime-1"), RequestFrame::Resources)
        .await
        .expect("control exchange succeeds");
    assert_eq!(actual, response);

    let provider = request_text(provider.await.expect("provider fixture completes"));
    assert!(provider
        .starts_with("GET /v1/sandboxes/runtime-1/endpoints/9090?use_server_proxy=true HTTP/1.1"));
    assert!(provider
        .to_ascii_lowercase()
        .contains("open-sandbox-api-key: provider-secret"));

    let runtime = request_text(runtime.await.expect("runtime fixture completes"));
    assert!(runtime.starts_with("POST /proxy/3001/runtime/control HTTP/1.1"));
    assert!(runtime
        .to_ascii_lowercase()
        .contains("open-sandbox-api-key: provider-secret"));
    let body = runtime.split_once("\r\n\r\n").expect("control body").1;
    assert_eq!(
        serde_json::from_str::<RequestFrame>(body).unwrap(),
        RequestFrame::Resources
    );
}

#[tokio::test]
async fn runtime_control_redirects_are_not_followed_or_given_authentication() {
    let redirect_target = listener().await;
    let redirect_address = redirect_target
        .local_addr()
        .expect("redirect target address");
    let redirect = http_response(
        "302 Found",
        &[
            ("Location", format!("http://{redirect_address}/stolen")),
            ("Content-Length", "0".into()),
        ],
        &[],
    );
    let (transport, provider, runtime) = transport_fixture(redirect).await;

    let error = transport
        .exchange(&RuntimeHandle::new("runtime-1"), RequestFrame::Resources)
        .await
        .expect_err("runtime redirect is refused");
    assert!(matches!(error, AdapterError::Rejected(detail) if detail.contains("302")));
    provider.await.expect("provider fixture completes");
    let runtime = request_text(runtime.await.expect("runtime fixture completes"));
    assert!(runtime
        .to_ascii_lowercase()
        .contains("open-sandbox-api-key: provider-secret"));
    assert!(
        tokio::time::timeout(Duration::from_millis(200), redirect_target.accept())
            .await
            .is_err(),
        "runtime redirect target unexpectedly received the credential"
    );
}

#[tokio::test]
async fn runtime_control_responses_are_bounded_before_allocation() {
    let oversized = http_response(
        "200 OK",
        &[("Content-Length", (384_u64 * 1024 * 1024 + 1).to_string())],
        &[],
    );
    let (transport, provider, runtime) = transport_fixture(oversized).await;

    let error = transport
        .exchange(&RuntimeHandle::new("runtime-1"), RequestFrame::Resources)
        .await
        .expect_err("oversized runtime response is refused");
    assert!(matches!(error, AdapterError::Rejected(detail) if detail.contains("size limit")));
    provider.await.expect("provider fixture completes");
    runtime.await.expect("runtime fixture completes");
}

#[tokio::test]
async fn runtime_binding_selects_only_the_accepted_sandbox_store() {
    let provider = listener().await;
    let provider_address = provider.local_addr().expect("provider address");
    drop(provider);
    let factory = Arc::new(RecordingFactory::default());
    let erased: Arc<dyn CheckpointArchiveStoreFactory> = factory.clone();
    let transport = ProviderHostedControlTransport::new(
        SandboxClient::new(format!("http://{provider_address}"), "provider-secret"),
        erased,
    )
    .expect("transport builds");
    let handle = RuntimeHandle::new("runtime-1");

    transport
        .bind_runtime(&handle, "sandbox-a")
        .await
        .expect("first binding succeeds");
    transport
        .bind_runtime(&handle, "sandbox-a")
        .await
        .expect("same binding is idempotent");
    let error = transport
        .bind_runtime(&handle, "sandbox-b")
        .await
        .expect_err("cross-sandbox rebinding is refused");

    assert!(matches!(error, AdapterError::Rejected(_)));
    assert_eq!(
        factory
            .sandboxes
            .lock()
            .expect("factory record lock")
            .as_slice(),
        ["sandbox-a"]
    );
}

#[tokio::test]
async fn hosted_capacity_recovery_maps_an_unadvertised_runtime_endpoint_to_unsupported() {
    let response = http_response("404 Not Found", &[("Content-Length", "0".into())], &[]);
    let (transport, provider, runtime) = transport_fixture(response).await;
    let request = crate::agent_capacity_recovery::tests::request(None);
    let (updates, _received) = tokio::sync::mpsc::channel(1);

    assert!(matches!(
        transport
            .reattach_existing_capacity_recovery(&request, updates)
            .await,
        Err(crate::agent_capacity_recovery::CapacityRecoveryError::Unsupported)
    ));
    provider.await.expect("provider fixture completes");
    runtime.await.expect("runtime fixture completes");
}

async fn receive_websocket_json(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
) -> serde_json::Value {
    let frame = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .expect("ACP request arrives")
        .expect("ACP socket remains open")
        .expect("ACP request is readable");
    let Message::Binary(frame) = frame else {
        panic!("expected binary ACP request, received {frame:?}");
    };
    assert_eq!(frame.first(), Some(&super::super::acp::LANE_CONVERSATION));
    serde_json::from_slice(&frame[1..]).expect("ACP request is JSON")
}

#[path = "provider_permission_open_tests.rs"]
mod permission_open_tests;
#[path = "provider_raw_output_open_tests.rs"]
mod raw_output_open_tests;

#[path = "provider_open_tests.rs"]
mod open_tests;

#[path = "provider_status_compatibility_tests.rs"]
mod status_compatibility;

#[path = "provider_output_json_tests.rs"]
mod output_json_policy;
